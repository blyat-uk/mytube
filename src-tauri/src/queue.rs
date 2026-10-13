use anyhow::{anyhow, Result};
use std::collections::{BTreeSet, HashMap};
use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::sync::{watch, Mutex, OwnedSemaphorePermit, Semaphore};

use crate::db::Db;
use crate::models::{DownloadProgress, DownloadState, DownloadStateEvent};
use crate::quality::{self, Quality};
use crate::tools::Tools;
use crate::{config, proc, ytdlp};

/// Keeps the last `max` characters, never splitting a UTF-8 boundary.
pub fn tail(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    s.chars().skip(s.chars().count() - max).collect()
}

/// Output paths reserved by in-flight downloads.
///
/// Two videos can resolve to the same filename, and the file does not exist
/// until yt-dlp finishes writing it — so disk checks alone would let two
/// concurrent jobs pick the same name. Claiming is done under one lock, so the
/// check and the reservation cannot interleave.
#[derive(Default)]
pub struct PathClaims {
    inner: std::sync::Mutex<std::collections::HashSet<PathBuf>>,
}

impl PathClaims {
    pub fn is_claimed(&self, p: &Path) -> bool {
        self.inner.lock().unwrap().contains(p)
    }
    pub fn claim(&self, p: PathBuf) -> bool {
        self.inner.lock().unwrap().insert(p)
    }
    pub fn release(&self, p: &Path) {
        self.inner.lock().unwrap().remove(p);
    }

    /// Picks a free path and reserves it atomically. `on_disk` reports files
    /// that already exist.
    pub fn claim_unique(&self, intended: &Path, on_disk: impl Fn(&Path) -> bool) -> PathBuf {
        self.claim_unique_with(intended, None, on_disk).0
    }

    /// `claim_unique` for a download whose file passes through a second name
    /// first: `-x --audio-format mp3` downloads `Name.m4a` and converts it to
    /// `Name.mp3` (see `quality::final_ext`). Both names have to be free --
    /// yt-dlp would take an existing `Name.m4a` for its own finished download
    /// and convert *that* -- and both are reserved, so the cancel sweep may
    /// safely delete the intermediate one. Returns the chosen path and, when
    /// `via_ext` is given, its intermediate twin.
    pub fn claim_unique_with(&self, intended: &Path, via_ext: Option<&str>,
                             on_disk: impl Fn(&Path) -> bool) -> (PathBuf, Option<PathBuf>) {
        let mut guard = self.inner.lock().unwrap();
        let taken = |c: &Path| guard.contains(c) || on_disk(c);
        let chosen = ytdlp::unique_path(intended, |c| {
            taken(c) || via_ext.is_some_and(|e| taken(&c.with_extension(e)))
        });
        let via = via_ext.map(|e| chosen.with_extension(e));
        guard.insert(chosen.clone());
        if let Some(v) = &via {
            guard.insert(v.clone());
        }
        (chosen, via)
    }
}

/// Where the finished file is meant to land: the probe's path, with the
/// extension post-processing will give it (`quality::final_ext`), since the
/// probe prints the name from *before* it. Also returns the probe's own
/// extension when that differs -- the name the file passes through on the way
/// -- for `claim_unique_with`.
fn intended_path(probed: &Path, final_ext: Option<&str>) -> (PathBuf, Option<String>) {
    let Some(ext) = final_ext else { return (probed.to_path_buf(), None) };
    let own = probed.extension().and_then(|e| e.to_str()).map(str::to_string);
    let via = own.filter(|o| o != ext);
    (probed.with_extension(ext), via)
}

/// One in-flight download's stop button.
///
/// The cancelled flag and the kill handle live behind one lock because they
/// are read and written together. A cancel arriving between `run_one` asking
/// "am I cancelled?" and handing over the yt-dlp it has just spawned would
/// otherwise find no process to kill and leave no word that it happened -- and
/// the download would run to completion with nothing left to stop it.
///
/// `K` is the handle that kills yt-dlp's tree: [`proc::KillTree`] for real,
/// and a plain number in the tests, which check the handover and not the kill.
///
/// Signalling yt-dlp alone is not enough: it shells out to ffmpeg to merge the
/// separate video and audio streams, and an orphaned ffmpeg carries on writing
/// the very file the cancel is about to delete. Every download is spawned
/// through `proc::spawn_tree` -- its own process group on unix, its own Job
/// Object on Windows -- so that one kill reaches the lot.
struct Job<K = proc::KillTree> {
    inner: std::sync::Mutex<JobInner<K>>,
}

impl<K> Default for Job<K> {
    fn default() -> Self {
        Job {
            inner: std::sync::Mutex::new(JobInner { cancelled: false, tree: None, spawned: false }),
        }
    }
}

struct JobInner<K> {
    cancelled: bool,
    /// The tree yt-dlp is running in, for as long as it is running. A job can
    /// run yt-dlp twice -- again without subtitles, when a track would not
    /// download (`run_one`) -- but one after the other, never side by side:
    /// the second is only `started` once the first is `reaped`.
    tree: Option<K>,
    /// Set when yt-dlp is spawned and never cleared. `tree` answers "is there
    /// something to kill?"; this answers "might there be a part file on disk?",
    /// which stays true after the child is reaped and is what tells `cancel` to
    /// wait for the job to clear up rather than abort it where it stands.
    spawned: bool,
}

/// What `cancel` needs to know about the job it just stopped.
struct Stop<K> {
    tree: Option<K>,
    spawned: bool,
}

impl<K> Job<K> {
    fn is_cancelled(&self) -> bool {
        self.inner.lock().unwrap().cancelled
    }

    /// Marks the job cancelled and hands back the tree to kill, once.
    fn cancel(&self) -> Stop<K> {
        let mut g = self.inner.lock().unwrap();
        g.cancelled = true;
        Stop {
            // Taken, not copied: a second cancel must not signal a pid the
            // kernel has since handed to somebody else.
            tree: g.tree.take(),
            spawned: g.spawned,
        }
    }

    /// Adopts the tree yt-dlp was spawned into. Returns false if a cancel got
    /// in first, in which case the caller must kill what it has just spawned
    /// itself -- nobody else now holds a handle to it. That includes a cancel
    /// landing between a job's two runs, while nothing was running to kill.
    fn started(&self, tree: K) -> bool {
        let mut g = self.inner.lock().unwrap();
        g.spawned = true;
        if g.cancelled {
            return false;
        }
        g.tree = Some(tree);
        true
    }

    /// yt-dlp has exited; there is no longer a tree worth killing.
    fn reaped(&self) {
        self.inner.lock().unwrap().tree = None;
    }

    /// Runs `f` -- a write of the row -- unless the job has been cancelled,
    /// and holds the job's lock while it does. Returns `None`, having run
    /// nothing, for a cancelled job.
    ///
    /// Holding the lock is the point. `Queue::cancel` marks the job cancelled
    /// (which takes this lock) *before* it writes `None`, so a job's own write
    /// either lands first and is overwritten, or sees the cancel and never
    /// happens. Checking `is_cancelled` and then writing would leave a gap in
    /// which a `Done` or `Failed` lands after the cancel's `None` -- a row
    /// claiming a file the cancel is about to delete.
    fn unless_cancelled<R>(&self, f: impl FnOnce() -> R) -> Option<R> {
        let g = self.inner.lock().unwrap();
        if g.cancelled {
            return None;
        }
        let r = f();
        drop(g);
        Some(r)
    }
}

/// What `Registry` needs of a tree handle: a way to kill it. The real one is
/// [`proc::KillTree`]; the tests use a number and kill nothing.
trait Kill: Send + Sync + 'static {
    fn kill(&self);
}

impl Kill for proc::KillTree {
    fn kill(&self) {
        proc::KillTree::kill(self)
    }
}

#[cfg(test)]
impl Kill for i32 {
    fn kill(&self) {}
}

/// Held by everything still working on a job's behalf -- its task, and the
/// sweep a cancelled job's `CleanupGuard` hands to a blocking thread. When the
/// last clone is dropped, the matching [`Gone`] resolves.
///
/// A `JoinHandle` cannot say this: the job's task can be over while the sweep
/// it started on a blocking thread is still deleting files, and a download of
/// the same video must not begin until that sweep has finished too.
#[derive(Clone)]
struct Alive(#[allow(dead_code)] Arc<watch::Sender<()>>);

/// Resolves once every [`Alive`] of its pair has been dropped.
#[derive(Clone)]
struct Gone(watch::Receiver<()>);

fn lifeline() -> (Alive, Gone) {
    let (tx, rx) = watch::channel(());
    (Alive(Arc::new(tx)), Gone(rx))
}

impl Gone {
    async fn wait(mut self) {
        // Nothing is ever sent, so this only returns once the sender is gone.
        while self.0.changed().await.is_ok() {}
    }

    fn same(&self, other: &Gone) -> bool {
        self.0.same_channel(&other.0)
    }
}

/// The order downloads take their slots in: exactly the order they were
/// queued.
///
/// The semaphore never promised that on its own. It is fair to whoever is
/// already waiting on it, but every download is a task of its own, and which
/// of two tasks spawned a moment apart gets to `acquire` first is the
/// runtime's business -- on a multi-threaded one, either. So "Download all" on
/// a series could start part 3 before part 1, and a video queued last could
/// overtake a whole backlog. The line decides who waits on the semaphore: one
/// download at a time, the one at the front.
///
/// A place is a number, handed out under the registry's lock in the same
/// breath as the row's `Queued` (`Registry::enqueue`), so the order of places
/// is the order the rows say. Being served means waiting for a slot, taking
/// it, and stepping out of the way: the next place is served while this
/// download runs.
///
/// Leaving is dropping the [`Place`] -- served, or not. That is how a download
/// that never gets its turn gets out of the way: cancelled where it stands
/// (`Registry::cancel` aborts a queued job), or torn down by a panic. A place
/// left before its number comes up cannot simply wave the next one through,
/// because places ahead of it may still be waiting and the next must not
/// overtake those; it is noted instead, and skipped when its number comes up.
struct Line {
    turns: std::sync::Mutex<Turns>,
    /// The number being served, for the places waiting on it.
    serving: watch::Sender<u64>,
}

struct Turns {
    /// The next number to hand out.
    issued: u64,
    /// The number being served. Only that place's own leaving moves it on,
    /// so it never passes a place still in the line.
    serving: u64,
    /// Places left before their number came up.
    left: BTreeSet<u64>,
}

impl Line {
    fn new() -> Arc<Line> {
        Arc::new(Line {
            turns: std::sync::Mutex::new(Turns { issued: 0, serving: 0, left: BTreeSet::new() }),
            serving: watch::channel(0).0,
        })
    }

    /// Nothing panics under this lock, and every change is whole before it is
    /// let go, so a poisoned one is still good -- and a `Place` dropped while
    /// unwinding must still be able to leave.
    fn turns(&self) -> std::sync::MutexGuard<'_, Turns> {
        self.turns.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// A place at the back of the line.
    fn join(self: &Arc<Self>) -> Place {
        let mut t = self.turns();
        let n = t.issued;
        t.issued += 1;
        Place { line: self.clone(), n }
    }

    /// Place `n` is done with the line, served or not.
    fn leave(&self, n: u64) {
        let mut t = self.turns();
        if t.serving != n {
            t.left.insert(n);
            return;
        }
        let mut next = n + 1;
        while t.left.remove(&next) {
            next += 1;
        }
        t.serving = next;
        // Sent under the lock, so the number only ever goes up.
        self.serving.send_replace(next);
    }
}

/// One download's place in the [`Line`]. Dropping it leaves the line.
struct Place {
    line: Arc<Line>,
    n: u64,
}

impl Place {
    /// Waits until every download queued before this one has its slot, takes
    /// one from `slots`, and gives the place up -- in that order, so the next
    /// download is only let at the semaphore once this one is through it.
    /// `None` only for a closed semaphore, which nothing closes.
    async fn admit(self, slots: &Arc<Semaphore>) -> Option<OwnedSemaphorePermit> {
        // `>=` and `==` are the same here -- only this place's own leaving can
        // move the number past it -- but `>=` could not hang if that ever
        // changed. The sender lives in the line this place holds, so the wait
        // cannot fail; the `Ref` it returns is let go at once, because a leave
        // elsewhere needs to write the value it borrows.
        let _ = self.line.serving.subscribe().wait_for(|&s| s >= self.n).await;
        slots.clone().acquire_owned().await.ok()
    }
}

impl Drop for Place {
    fn drop(&mut self) {
        self.line.leave(self.n);
    }
}

/// Does `name` look like something yt-dlp wrote on the way to `out_path`?
///
/// Matched by shape rather than by a bare "starts with the stem" test, because
/// two real uploads can be `Part 1.mkv` and `Part 1.5.mkv` -- the second begins
/// with the first's stem followed by a dot, and deleting it would cost somebody
/// a video they had downloaded. Only the shapes yt-dlp actually produces count:
/// the merged file itself, the separate streams (`.f137.mp4`) with their
/// in-progress and fragment files (`.part`, `.part-FragN`) and resume state
/// (`.ytdl`), ffmpeg's merge target (`.temp.mkv`), and the thumbnail and
/// subtitle tracks (`.en.srt`) fetched for embedding, and the chapter list
/// written for ffmpeg to embed (`.meta` -- FFmpegMetadataPP's
/// `replace_extension(filename, 'meta')`). Format ids are matched as digits,
/// which is what YouTube uses.
fn is_leftover(out_path: &Path, name: &str) -> bool {
    if out_path.file_name().and_then(|n| n.to_str()) == Some(name) {
        return true;
    }
    let Some(rest) = after_stem(out_path, name) else {
        return false;
    };
    rest.ends_with(".part")
        || rest.ends_with(".ytdl")
        || rest.contains(".part-Frag")
        || rest.starts_with("temp.")
        || matches!(rest, "jpg" | "png" | "webp" | "meta")
        || is_format_stream(rest)
        || is_subtitle_track(rest)
}

/// What follows `<stem>.` in `name`, if `name` begins with `out_path`'s stem
/// and a dot: the part of a sidecar's name that says what it is.
fn after_stem<'a>(out_path: &Path, name: &'a str) -> Option<&'a str> {
    let stem = out_path.file_stem().and_then(|n| n.to_str())?;
    name.strip_prefix(stem)?.strip_prefix('.')
}

/// `f137.mp4`, `f251.webm` -- one of the streams yt-dlp downloads to merge.
fn is_format_stream(rest: &str) -> bool {
    let Some((id, ext)) = rest.strip_prefix('f').and_then(|t| t.split_once('.')) else {
        return false;
    };
    !id.is_empty() && id.chars().all(|c| c.is_ascii_digit()) && !ext.is_empty()
}

/// The formats a subtitle track can arrive in. `--sub-format srt/best` asks
/// for srt, and a track YouTube offers no srt of falls back to its best.
const SUBTITLE_EXTS: [&str; 9] =
    ["srt", "vtt", "ass", "ssa", "ttml", "srv1", "srv2", "srv3", "json3"];

/// `en.srt`, `de-DE.srt`, `es-419.vtt` -- one subtitle track fetched to embed,
/// written as `<stem>.<lang>.<format>`. yt-dlp makes the language out of
/// YouTube's track id with every dot turned into a hyphen (`get_lang_code`,
/// checked in yt-dlp 2026.08.19: the named track `.en.nP7-2PuUl7o` becomes
/// `en-nP7-2PuUl7o`), so it never holds one; that id is shaped like a video
/// id, so `_` is allowed beside letters, digits and `-`. A language begins
/// with a letter, which is what keeps `Part 1.5.srt` -- the subtitles of the
/// video `Part 1.5` -- from passing for a track of `Part 1`.
fn is_subtitle_track(rest: &str) -> bool {
    let Some((lang, ext)) = rest.split_once('.') else {
        return false;
    };
    lang.starts_with(|c: char| c.is_ascii_alphabetic())
        && lang.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        && SUBTITLE_EXTS.contains(&ext)
}

/// A subtitle track of `out_path`'s, finished or in progress (`.en.srt.part`):
/// what a run that failed over one of them can have left, and all that is
/// cleared away before the run without them (`run_one`). Nothing else -- the
/// video's own part files are not that run's to touch.
fn is_subtitle_leftover(out_path: &Path, name: &str) -> bool {
    after_stem(out_path, name)
        .is_some_and(|rest| is_subtitle_track(rest.strip_suffix(".part").unwrap_or(rest)))
}

/// Deletes everything in `out_path`'s directory that `wanted` says a download
/// of it left behind -- `is_leftover` after a cancel -- in one pass. Returns
/// what it could not delete.
fn remove_leftovers(out_path: &Path, wanted: fn(&Path, &str) -> bool) -> Vec<PathBuf> {
    let Some(dir) = out_path.parent() else { return Vec::new() };
    let Ok(entries) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut stuck = Vec::new();
    for e in entries.flatten() {
        let name = e.file_name();
        if name.to_str().is_some_and(|n| wanted(out_path, n)) {
            if let Err(err) = std::fs::remove_file(e.path()) {
                if err.kind() != std::io::ErrorKind::NotFound {
                    stuck.push(e.path());
                }
            }
        }
    }
    stuck
}

/// How often, and how many times, a leftover that would not delete is tried
/// again. Windows only: a file another process still has open cannot be
/// unlinked there, and a yt-dlp or ffmpeg that has been told to die may take
/// a moment to let go even once the job reports it empty (antivirus scanners
/// open freshly written files too). Unix unlinks an open file without fuss.
#[cfg(windows)]
const SWEEP_RETRY: std::time::Duration = std::time::Duration::from_millis(200);
#[cfg(windows)]
const SWEEP_ATTEMPTS: u32 = 10;

/// `remove_leftovers` for each path, then on Windows the files that would not
/// go are tried again a few times. Blocking all the way -- a directory listing,
/// unlinks, and on Windows sleeps between retries -- so it runs on a blocking
/// thread, never a runtime worker: the download directory is as often as not a
/// spun-down HDD or a network mount, and one `read_dir` there can take
/// seconds, which on a worker stalls every other task scheduled on it.
fn sweep_blocking(out_paths: &[PathBuf], wanted: fn(&Path, &str) -> bool) {
    #[allow(unused_mut, unused_variables)]
    let mut stuck: Vec<PathBuf> =
        out_paths.iter().flat_map(|p| remove_leftovers(p, wanted)).collect();
    #[cfg(windows)]
    for _ in 0..SWEEP_ATTEMPTS {
        if stuck.is_empty() {
            break;
        }
        std::thread::sleep(SWEEP_RETRY);
        stuck = remove_stuck(stuck);
    }
}

/// `sweep_blocking` of everything a cancel clears, awaited from a blocking
/// thread.
async fn sweep_leftovers(out_paths: Vec<PathBuf>) {
    let _ = tokio::task::spawn_blocking(move || sweep_blocking(&out_paths, is_leftover)).await;
}

/// `sweep_blocking` of the subtitle tracks alone, awaited from a blocking
/// thread: the run without subtitles would leave them where they are, and
/// nothing would ever clear them away after a download that ends `Done`.
async fn sweep_subtitles(out_paths: Vec<PathBuf>) {
    let _ = tokio::task::spawn_blocking(move || sweep_blocking(&out_paths, is_subtitle_leftover))
        .await;
}

/// Runs blocking filesystem work off the runtime's workers; see
/// `sweep_blocking` for why that matters on this app's download directories.
async fn blocking<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> Result<T> {
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|e| anyhow!("a filesystem task failed: {e}"))
}

/// Tries each path once more; returns the ones still there.
#[cfg(windows)]
fn remove_stuck(paths: Vec<PathBuf>) -> Vec<PathBuf> {
    paths
        .into_iter()
        .filter(|p| match std::fs::remove_file(p) {
            Ok(()) => false,
            Err(e) => e.kind() != std::io::ErrorKind::NotFound,
        })
        .collect()
}

/// Clears up after a cancelled download, however `run_one` leaves.
///
/// This has to be a guard rather than a branch at the end of the function,
/// because a cancel can land at any instant -- including after the file is
/// written and the row says `Done`, with `Queue::cancel` about to overwrite it
/// with `None`. Wherever the two interleave, the end state is the same: no row,
/// no file. It is declared *before* the child so that it drops after it -- locals
/// drop in reverse -- which means yt-dlp is dead before anything is unlinked.
///
/// Only a cancel clears up. A *failed* download keeps its `.part` files on
/// purpose: yt-dlp resumes from them, and `claim_unique` hands the retry the
/// same path because the merged file it would collide with does not exist yet.
struct CleanupGuard {
    job: Arc<Job>,
    /// The file the download is to produce, and the intermediate name an
    /// audio conversion passes through (`intended_path`), when there is one.
    out_paths: Vec<PathBuf>,
    print_file: PathBuf,
    /// The job's claim on `out_paths`. Owned here so that a cancelled job
    /// keeps its names reserved until the sweep below has finished with them:
    /// released any earlier, another video of the same title could claim the
    /// name and start writing a `.part` file the sweep then deletes.
    claim: Option<ClaimGuard>,
    /// Keeps the job counted as still clearing up while the sweep runs on a
    /// blocking thread -- see `Alive`.
    alive: Alive,
}
impl Drop for CleanupGuard {
    fn drop(&mut self) {
        // Removed on every exit path, not just cancellation: an aborted job
        // used to leave one of these in /tmp for the life of the machine.
        // Removed here and now, not on the blocking thread below: a Retry of a
        // *failed* download starts at once and waits for nothing, and a late
        // removal could delete the file its own yt-dlp has just written.
        let _ = std::fs::remove_file(&self.print_file);
        let claim = self.claim.take();
        if !self.job.is_cancelled() {
            drop(claim);
            return;
        }
        // Normally `run_one` has already swept, with the waits it needs on
        // Windows, and this pass finds nothing; it is the backstop for the
        // exits that skip that -- an abort, an error, a late cancel. It lists
        // the download directory, so it goes to a blocking thread too: this
        // runs wherever the task was dropped, which is a runtime worker. A
        // download of the same video waits for it through `alive` (see
        // `Registry`), and the claim is only let go once it is done.
        let out_paths = std::mem::take(&mut self.out_paths);
        let alive = self.alive.clone();
        let work = move || {
            let _alive = alive;
            sweep_blocking(&out_paths, is_leftover);
            drop(claim);
        };
        match tokio::runtime::Handle::try_current() {
            Ok(h) => drop(h.spawn_blocking(work)),
            Err(_) => work(),
        }
    }
}

/// Releases claimed paths however the job ends, including on panic or abort.
struct ClaimGuard {
    claims: Arc<PathClaims>,
    paths: Vec<PathBuf>,
}
impl Drop for ClaimGuard {
    fn drop(&mut self) {
        for p in &self.paths {
            self.claims.release(p);
        }
    }
}

/// How long a cancelled download gets, in the background, to kill yt-dlp,
/// reap it and delete its part files before its task is aborted. That is
/// milliseconds of real work; the allowance is wide only so an unlink stalled
/// on a busy disk is not cut short. `cancel` itself no longer waits for any of
/// it -- see `Registry::cancel`.
const CLEANUP_GRACE: Duration = Duration::from_secs(15);

/// How long a cancelled download waits for every process in its tree to be
/// gone before it deletes anything. Windows only; see `KillTree::wait_empty`.
const TREE_EXIT_GRACE: Duration = Duration::from_secs(5);

/// The most often one download's `download://progress` is emitted. yt-dlp
/// runs with `--newline` and prints a line per progress tick -- dozens a second
/// on a fast connection, each one an IPC message and a React re-render of the
/// grid -- while a bar and a speed read the same at four updates a second.
const PROGRESS_EVERY: Duration = Duration::from_millis(250);

/// Thins one download's progress lines to one per `PROGRESS_EVERY`, without
/// ever leaving the bar on a stale value: the first line always goes, so does
/// 100%, and so does a percentage that went *down* -- yt-dlp fetches the video
/// and audio streams one after the other and starts the second from 0%, and
/// dropping that line would leave the bar at 100% for the whole second stream.
/// What is held back when the output ends is flushed by the caller.
#[derive(Default)]
struct ProgressThrottle {
    /// When the last line went out, and its percentage.
    last: Option<(Instant, f64)>,
}

impl ProgressThrottle {
    fn admit(&mut self, now: Instant, percent: f64) -> bool {
        let go = match self.last {
            None => true,
            Some((at, was)) => {
                percent >= 100.0 || percent < was || now.duration_since(at) >= PROGRESS_EVERY
            }
        };
        if go {
            self.last = Some((now, percent));
        }
        go
    }
}

/// The two row writes the queue makes on its own account -- `Queued` and a
/// cancel's `None` -- each followed by the `download://state` event that tells
/// the UI, and a read of where the row stands. A trait so `Registry` can be
/// tested without a Tauri runtime or a database.
trait Rows: Send + Sync + 'static {
    fn set(&self, video_id: &str, state: DownloadState) -> Result<()>;
    fn state(&self, video_id: &str) -> Option<DownloadState>;
}

/// Tells every view a row's download state moved. Also used by
/// `commands::delete_download`, the one write of the state outside the queue.
pub(crate) fn emit_state(app: &AppHandle, video_id: &str, state: DownloadState,
                         file_path: Option<String>, error: Option<String>) {
    let _ = app.emit(
        "download://state",
        DownloadStateEvent { video_id: video_id.to_string(), state, file_path, error },
    );
}

/// `Rows` for real: the database and the webview.
struct Ledger {
    db: Arc<Db>,
    app: AppHandle,
}

impl Rows for Ledger {
    fn set(&self, video_id: &str, state: DownloadState) -> Result<()> {
        self.db.set_download_state(video_id, state, None, None)?;
        emit_state(&self.app, video_id, state, None, None);
        Ok(())
    }
    fn state(&self, video_id: &str) -> Option<DownloadState> {
        self.db.get_video(video_id).ok().flatten().map(|v| v.download_state)
    }
}

/// One queued or running download.
struct Slot<K> {
    job: Arc<Job<K>>,
    abort: tokio::task::AbortHandle,
    gone: Gone,
}

struct Book<K> {
    /// Every download queued or running, by video id.
    jobs: HashMap<String, Slot<K>>,
    /// Cancelled downloads still clearing up, by video id: resolves once the
    /// job is gone and its sweep is done. A new download of the same video
    /// waits on it before it does anything at all -- the old sweep deletes by
    /// the old job's names, and the new one would otherwise be writing to
    /// those very names: the shared `mytube-{id}.path` print file, the same
    /// claimed output path, the same `.part` files it resumes from.
    cleaning: HashMap<String, Gone>,
}

/// Which downloads exist, and the rules for starting, finishing and
/// cancelling one. Kept apart from `Queue` so it can be driven in tests with
/// fake jobs, and generic over the kill handle for the same reason.
///
/// Everything is behind one *synchronous* lock, and nothing ever awaits while
/// holding it -- the compiler enforces that for the spawned tasks, since a std
/// guard is not `Send`. That is the fix for a deadlock `cancel` used to walk
/// into: it held the tokio-locked `running` map (under edition 2021 the guard
/// of an `if let` scrutinee lives for the whole `if let`/`else`) while waiting
/// for the job, whose own last act was to lock `running` -- so every cancel of
/// a running download hung for the full 15 s grace, and every enqueue, every
/// `is_active` and every other finishing job hung behind it.
///
/// The line has a lock of its own, taken *inside* the book's (a place is
/// joined under it, in `enqueue`) and never the other way round: leaving the
/// line takes the line's lock alone, so a place dropped by an abort or mid-
/// panic, wherever that happens, can never wait on a book somebody holds.
struct Registry<K = proc::KillTree> {
    book: std::sync::Mutex<Book<K>>,
    /// The order the downloads take their slots in -- see `Line`.
    line: Arc<Line>,
    /// One permit per download allowed to run at once. `Queue` holds the same
    /// semaphore, to grow it when the setting is raised.
    slots: Arc<Semaphore>,
}

impl<K: Kill> Registry<K> {
    fn new(slots: Arc<Semaphore>) -> Arc<Self> {
        Arc::new(Registry {
            book: std::sync::Mutex::new(Book { jobs: HashMap::new(), cleaning: HashMap::new() }),
            line: Line::new(),
            slots,
        })
    }

    /// A panic under the lock cannot leave the maps half-written -- every
    /// change is one insert or remove -- so a poisoned lock is still good.
    fn book(&self) -> std::sync::MutexGuard<'_, Book<K>> {
        self.book.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn is_active(&self, video_id: &str) -> bool {
        self.book().jobs.contains_key(video_id)
    }

    /// Writes `Queued` and starts `body` as the download's task. A video
    /// already queued or running is left alone.
    ///
    /// The row is written, the task spawned and the slot filed under the one
    /// lock, so a cancel can never find the row queued but no slot to cancel,
    /// and the task's `Downloading` can never be overwritten by a late
    /// `Queued`. The download's place in the line is taken under it too, so
    /// downloads get their slots in the order their rows read `Queued`.
    ///
    /// The task takes its slot, in line, and only then waits out any
    /// cancelled download of the same video still clearing up
    /// (`Book::cleaning`) -- before a single file is touched, which is what
    /// that wait is for. The other way round, a download waiting on a slow
    /// sweep (a spun-down disk, a tree Windows is slow to empty) would stand
    /// at the front of the line with every slot free, and nothing behind it
    /// could start; this way it costs one slot for as long as the sweep
    /// takes, and the line moves on. It cannot deadlock: the job it waits on
    /// had either spawned yt-dlp, and so already holds a slot and waits on
    /// neither the line nor the semaphore, or it had not, and `cancel`
    /// aborted it where it stood.
    fn enqueue<R, F, Fut>(self: &Arc<Self>, video_id: String, rows: &Arc<R>, body: F) -> Result<()>
    where
        R: Rows,
        F: FnOnce(Arc<Job<K>>, Alive) -> Fut,
        Fut: Future<Output = ()> + Send + 'static,
    {
        let mut book = self.book();
        if book.jobs.contains_key(&video_id) {
            return Ok(()); // already in flight
        }
        rows.set(&video_id, DownloadState::Queued)?;

        let job = Arc::new(Job::default());
        let (alive, gone) = lifeline();
        let pending = book.cleaning.get(&video_id).cloned();
        let place = self.line.join();
        let slots = self.slots.clone();
        let work = body(job.clone(), alive.clone());
        let finish = Finish { reg: self.clone(), video_id: video_id.clone(), job: job.clone() };
        let handle = tokio::spawn(async move {
            // Declared first so it is dropped last: the job is not gone until
            // everything else in here has been torn down.
            let _alive = alive;
            let _finish = finish;
            // The slot is held to the end, whichever way the task ends -- a
            // panic in the download included -- and given back before
            // `Finish` runs. Until it is in hand the task holds its place
            // instead, so an abort while it waits leaves the line.
            let Some(_slot) = place.admit(&slots).await else { return };
            if let Some(p) = pending {
                p.wait().await;
            }
            work.await;
        });
        book.jobs.insert(video_id, Slot { job, abort: handle.abort_handle(), gone });
        Ok(())
    }

    /// Stops a download and returns at once.
    ///
    /// Under the lock: the slot is taken out, the job marked cancelled, the
    /// row set to `None` and the UI told, and the video filed as clearing up.
    /// Then yt-dlp's tree is killed. What is left -- waiting for the job to
    /// sweep its part files -- happens on a task of its own, so the button
    /// answers immediately however slow the disk is. A download of the same
    /// video started meanwhile waits for that sweep (see `enqueue`).
    fn cancel<R: Rows>(self: &Arc<Self>, video_id: &str, rows: &Arc<R>) -> Result<()> {
        let mut book = self.book();
        let slot = book.jobs.remove(video_id);
        // Stop the job first, and mark it cancelled in the same breath, so
        // whatever it does next it knows to clear up rather than record a file
        // the library is about to forget about. `Job::cancel` waits out a
        // `Done` being written under `unless_cancelled`, which is what puts
        // the `None` below after it.
        let stop = slot.as_ref().map(|s| s.job.cancel());
        let written = rows.set(video_id, DownloadState::None);
        let (Some(slot), Some(stop)) = (slot, stop) else {
            return written;
        };
        let (alive, mine) = lifeline();
        let earlier = book.cleaning.insert(video_id.to_string(), mine.clone());
        drop(book);

        if let Some(tree) = &stop.tree {
            tree.kill();
        }
        if !stop.spawned {
            // Nothing has been spawned, so there is nothing on disk to wait
            // for -- and a queued job is waiting its turn behind other
            // downloads, which could be minutes. Dropping it leaves the line
            // (`Place`), so the downloads behind it move up rather than wait
            // for a turn nobody will take.
            slot.abort.abort();
        }

        let (reg, rows, video_id) = (self.clone(), rows.clone(), video_id.to_string());
        tokio::spawn(async move {
            let _alive = alive;
            // This job may itself have been waiting on an earlier cancel of
            // the same video; whoever waits on this one waits on that too.
            if let Some(e) = earlier {
                e.wait().await;
            }
            // Something may be on disk, so let the job sweep it up. Aborting
            // it outright (which is what cancel once did) skipped that cleanup
            // and, worse, left yt-dlp running: tokio does not kill a child when
            // its task is dropped, so it ran on to a full download that nothing
            // then recorded or deleted. The abort is now only for a job that
            // has not finished within the grace; dropping it kills the tree
            // (`TreeChild`) and runs `CleanupGuard`'s own sweep.
            if tokio::time::timeout(CLEANUP_GRACE, slot.gone.clone().wait()).await.is_err() {
                eprintln!("[mytube] {video_id}: a cancelled download outlived its grace; aborting it");
                slot.abort.abort();
                if tokio::time::timeout(CLEANUP_GRACE, slot.gone.wait()).await.is_err() {
                    eprintln!("[mytube] {video_id}: gave up waiting for a cancelled download to clear up");
                }
            }
            // A read and maybe a write of the row, under the book's lock: on a
            // blocking thread, so a database held by an import parks that and
            // not a runtime worker. `_alive` outlives it, so a download of the
            // same video still waits for the settle.
            let _ = tokio::task::spawn_blocking(move || reg.settle(&video_id, &mine, &*rows)).await;
        });
        written
    }

    /// A cancelled download has cleared up. Its record goes -- unless a later
    /// cancel of the same video has replaced it -- and the row is put back to
    /// `None` if it has somehow moved since the cancel wrote it: the backstop
    /// for a job write the cancel's `None` did not come after. Left alone when
    /// a new download of the video exists, because that one owns the row now
    /// (its `Queued` is already written), and when the row already reads
    /// `None`, since a second write would also clear a custom quality stored
    /// for the next download in the meantime.
    fn settle<R: Rows>(&self, video_id: &str, mine: &Gone, rows: &R) {
        let mut book = self.book();
        if book.cleaning.get(video_id).is_some_and(|g| g.same(mine)) {
            book.cleaning.remove(video_id);
        }
        if book.jobs.contains_key(video_id) {
            return;
        }
        if rows.state(video_id) != Some(DownloadState::None) {
            let _ = rows.set(video_id, DownloadState::None);
        }
    }

    /// The job's task is over: its slot goes, if it is still its own. After a
    /// cancel it is not -- the slot was taken out then, and a new download of
    /// the same video may be in it by now, which must not lose its entry to
    /// the old job's exit.
    fn finished(&self, video_id: &str, job: &Arc<Job<K>>) {
        let mut book = self.book();
        if book.jobs.get(video_id).is_some_and(|s| Arc::ptr_eq(&s.job, job)) {
            book.jobs.remove(video_id);
        }
    }
}

/// Calls `Registry::finished` however the task ends -- a panic included, which
/// would otherwise leave the video "active" for the life of the process, and
/// every later Download of it a silent no-op.
struct Finish<K: Kill> {
    reg: Arc<Registry<K>>,
    video_id: String,
    job: Arc<Job<K>>,
}
impl<K: Kill> Drop for Finish<K> {
    fn drop(&mut self) {
        self.reg.finished(&self.video_id, &self.job);
    }
}

pub struct Queue {
    db: Arc<Db>,
    app: AppHandle,
    sem: Arc<Semaphore>,
    permits: Arc<Mutex<usize>>,
    reg: Arc<Registry>,
    rows: Arc<Ledger>,
    claims: Arc<PathClaims>,
    tools: Arc<Tools>,
}

impl Queue {
    pub fn new(db: Arc<Db>, app: AppHandle, concurrency: usize, tools: Arc<Tools>) -> Self {
        let sem = Arc::new(Semaphore::new(concurrency.max(1)));
        Self {
            rows: Arc::new(Ledger { db: db.clone(), app: app.clone() }),
            db,
            app,
            tools,
            reg: Registry::new(sem.clone()),
            sem,
            permits: Arc::new(Mutex::new(concurrency.max(1))),
            claims: Arc::new(PathClaims::default()),
        }
    }

    /// Grows the semaphore when the user raises concurrency. Lowering it takes
    /// effect as running jobs finish, since permits cannot be revoked mid-flight.
    /// The new slots go to the downloads waiting longest: only the front of
    /// the line ever waits on the semaphore (`Line`), and each one through it
    /// lets the next one at it.
    pub async fn set_concurrency(&self, n: usize) {
        let n = n.clamp(1, 16);
        let mut cur = self.permits.lock().await;
        if n > *cur {
            self.sem.add_permits(n - *cur);
        }
        *cur = n;
    }

    /// Whether the video is queued or downloading right now.
    pub async fn is_active(&self, video_id: &str) -> bool {
        self.reg.is_active(video_id)
    }

    /// Returns as soon as the download is stopped and the row reads `None`;
    /// the part files are swept up behind it. See `Registry::cancel`.
    ///
    /// On a blocking thread, like `enqueue`: both write the row while holding
    /// the registry's lock, and a cancel also waits on the job's lock, which a
    /// job holds across its own row write -- any of which can wait on the
    /// database behind an import. `Registry` spawns its tasks from there, which
    /// works because the blocking pool runs inside the runtime.
    pub async fn cancel(&self, video_id: &str) -> Result<()> {
        let (reg, rows, id) = (self.reg.clone(), self.rows.clone(), video_id.to_string());
        blocking(move || reg.cancel(&id, &rows)).await?
    }

    pub async fn enqueue(&self, video_id: String, dir: String, template: String) -> Result<()> {
        let (db, app) = (self.db.clone(), self.app.clone());
        let claims = self.claims.clone();
        let tools = self.tools.clone();
        let vid = video_id.clone();
        let (reg, rows) = (self.reg.clone(), self.rows.clone());

        // Runs once the job holds its slot, taken in line by `Registry::enqueue`.
        blocking(move || reg.enqueue(video_id, &rows, move |job, alive| async move {
            if job.is_cancelled() {
                return;
            }
            let result =
                run_one(&db, &app, &tools, &vid, &dir, &template, &job, &alive, &claims).await;
            if let Err(e) = result {
                let msg = tail(&e.to_string(), 2000);
                // A cancelled job's error is the cancel's doing, and the row
                // is the cancel's to write.
                job.unless_cancelled(|| {
                    let _ = db.set_download_state(&vid, DownloadState::Failed, None, Some(&msg));
                    emit_state(&app, &vid, DownloadState::Failed, None, Some(msg.clone()));
                });
            }
        }))
        .await?
    }
}

#[allow(clippy::too_many_arguments)]
async fn run_one(
    db: &Db,
    app: &AppHandle,
    tools: &Tools,
    video_id: &str,
    dir: &str,
    template: &str,
    cancel: &Arc<Job>,
    alive: &Alive,
    claims: &Arc<PathClaims>,
) -> Result<()> {
    // Every touch of the download directory from here on goes through
    // `blocking`: see `sweep_blocking` for what a slow one does to a worker.
    // The print file in the temp dir and settings.json are left inline.
    let d = PathBuf::from(dir);
    blocking(move || std::fs::create_dir_all(d)).await??;
    let print_file = std::env::temp_dir().join(format!("mytube-{video_id}.path"));
    let _ = std::fs::remove_file(&print_file);

    let begun = cancel.unless_cancelled(|| -> Result<()> {
        db.set_download_state(video_id, DownloadState::Downloading, None, None)?;
        emit_state(app, video_id, DownloadState::Downloading, None, None);
        Ok(())
    });
    match begun {
        None => return Ok(()),
        Some(r) => r?,
    }

    // One yt-dlp for both phases, resolved now rather than at enqueue so a job
    // that sat in the queue gets whatever is installed when it starts. Its
    // lease is held until the download is reaped: the updater never swaps
    // yt-dlp while a job is using it. Not available yet is an ordinary failure.
    let settings = config::load().unwrap_or_default();
    let inv = tools.ytdlp(&settings).await?;

    // The quality is settled now too, for the same reason: a job queued before
    // Settings changed follows the new default. A video's own quality, from
    // "Download (custom)…", wins; an unreadable one falls back to Settings.
    let quality: Quality = db.download_quality(video_id).ok().flatten()
        .unwrap_or_else(|| settings.quality.clone());

    // Phase 1: resolve the output template to a concrete path.
    let probe = ytdlp::probe(&inv, &quality, &ytdlp::watch_url(video_id), dir, template).await?;
    let probed = PathBuf::from(&probe.intended_path);
    if probed.as_os_str().is_empty() {
        return Err(anyhow!("yt-dlp could not resolve an output filename"));
    }
    let (intended, via_ext) = intended_path(&probed, quality::final_ext(&quality));

    // Phase 2: claim a free path, appending " (N)" if this name is taken. The
    // disk checks run inside the claim's lock, as they always have -- that is
    // what makes the check and the reservation one step -- but on a blocking
    // thread. The guard is built there too: a cancel may abort this task while
    // the claim is in flight, and the guard then drops with the blocking
    // task's unwanted result instead of leaking the name for good.
    let (out_path, out_paths, claim) = {
        let claims = claims.clone();
        blocking(move || {
            let (out, via) = claims.claim_unique_with(&intended, via_ext.as_deref(),
                                                      |c| c.exists());
            let paths: Vec<PathBuf> = std::iter::once(out.clone()).chain(via).collect();
            let claim = ClaimGuard { claims, paths: paths.clone() };
            (out, paths, claim)
        })
        .await?
    };

    // The cleanup guard is declared before the children so that it drops
    // *after* them: yt-dlp is dead before a cancelled job unlinks anything.
    // Each child is owned by the `run_ytdlp` future awaited below, a temporary
    // of a later statement, so an aborted task drops -- and kills -- whichever
    // run is live first. The guard owns the claim, so the names stay reserved
    // until its sweep is done.
    let _cleanup = CleanupGuard {
        job: cancel.clone(),
        out_paths: out_paths.clone(),
        print_file: print_file.clone(),
        claim: Some(claim),
        alive: alive.clone(),
    };
    if let Some(parent) = out_path.parent().map(Path::to_path_buf) {
        blocking(move || std::fs::create_dir_all(parent)).await??;
    }

    let mut ran =
        run_ytdlp(app, &inv.runner, &quality, video_id, &out_path, &print_file, cancel).await?;

    // Subtitles are a preference and never a reason to fail a download, but
    // yt-dlp fetches them before the video and one track it cannot fetch fails
    // the whole run: `_write_subtitles` raises unless `--ignore-errors`. Seen
    // 2026-10-12 (yt-dlp 2026.10.10): `ERROR: Unable to download video
    // subtitles for 'de-DE': HTTP Error 429: Too Many Requests`. So the
    // download runs once more without them, and that run's outcome is final.
    // The tracks that did arrive go first: no run will embed them now. A
    // cancel in the meantime is caught by `Job::started` refusing the second
    // yt-dlp, which `run_ytdlp` then kills.
    if retry_without_subtitles(&quality, cancel.is_cancelled(), ran.status.success(), &ran.stderr) {
        let why = ran.stderr.lines().find(|l| ytdlp::is_subtitle_failure(l)).unwrap_or_default();
        eprintln!("[mytube] {video_id}: {why}; downloading again without subtitles");
        sweep_subtitles(out_paths.clone()).await;
        let plain = Quality { subtitles: false, ..quality.clone() };
        ran = run_ytdlp(app, &inv.runner, &plain, video_id, &out_path, &print_file, cancel).await?;
    }
    let Ran { status, stderr: stderr_text, tree } = ran;

    // The part files are deleted here, before the task ends, because that is
    // what a new download of the same video is waiting for (`Book::cleaning`);
    // the row is left alone because `Registry::cancel` owns it. On Windows the
    // leader being reaped does not mean the tree is: the kill has only been
    // *started* for the real yt-dlp and its ffmpeg, and their open handles make
    // a part file undeletable until they are gone -- so wait for the job to
    // empty first (bounded, well inside `CLEANUP_GRACE`), then sweep with
    // retries. `_cleanup` makes one more pass on the way out, which finds
    // nothing. On unix both steps are what they always were: no wait, one pass.
    if cancel.is_cancelled() {
        if !tree.wait_empty(TREE_EXIT_GRACE).await {
            eprintln!("[mytube] {video_id}: yt-dlp's processes outlived the cancel's grace; sweeping anyway");
        }
        sweep_leftovers(out_paths.clone()).await;
        return Ok(());
    }

    if !status.success() {
        let msg = stderr_text
            .lines()
            .rev()
            .find(|l| l.contains("ERROR"))
            .map(str::to_string)
            .unwrap_or_else(|| tail(stderr_text.trim(), 2000));
        return Err(anyhow!(
            "{}",
            if msg.is_empty() {
                format!("yt-dlp exited with {status}")
            } else {
                msg
            }
        ));
    }

    // yt-dlp appends to this file with `open(..., 'a', encoding='utf-8',
    // newline='')` and ends each value with `os.linesep` (checked in
    // `YoutubeDL._forceprint`, yt-dlp 2026.08.19) -- UTF-8 on every OS,
    // whatever `--encoding` says, and `\r\n` on Windows. So: read as UTF-8,
    // take the last line, trim the `\r`.
    let pf = print_file.clone();
    let path = blocking(move || -> Result<String> {
        let path = std::fs::read_to_string(&pf)
            .map_err(|_| anyhow!("yt-dlp finished but reported no output file"))?
            .lines()
            .last()
            .unwrap_or_default()
            .trim()
            .to_string();
        if path.is_empty() || !Path::new(&path).exists() {
            return Err(anyhow!("yt-dlp finished but the output file is missing"));
        }
        Ok(path)
    })
    .await??;

    // Under the job's lock: a cancel that got in first means the row is not
    // ours to write, and `_cleanup` is about to delete the file; one that
    // comes after finds `Done` written and overwrites it with `None`.
    let recorded = cancel.unless_cancelled(|| -> Result<()> {
        db.set_download_state(video_id, DownloadState::Done, Some(&path), None)?;
        emit_state(app, video_id, DownloadState::Done, Some(path.clone()), None);
        Ok(())
    });
    recorded.unwrap_or(Ok(()))
}

/// Whether a run of yt-dlp that has just ended is to be run again without
/// subtitles: it failed, over a subtitle track, with subtitles asked of it,
/// and not because it was cancelled. False for the run without them, so there
/// is never a third.
fn retry_without_subtitles(q: &Quality, cancelled: bool, success: bool, stderr: &str) -> bool {
    !cancelled && !success && quality::embeds_subtitles(q) && ytdlp::is_subtitle_failure(stderr)
}

/// What one run of yt-dlp came to.
struct Ran {
    status: std::process::ExitStatus,
    stderr: String,
    /// The tree it ran in, for a cancel to wait on before it sweeps.
    tree: proc::KillTree,
}

/// One run of yt-dlp for `run_one`: spawned into a tree of its own, handed to
/// the job, its progress relayed to the views, and waited for. Its stderr comes
/// back whole, for `run_one` to judge.
async fn run_ytdlp(app: &AppHandle, runner: &ytdlp::Runner, quality: &Quality, video_id: &str,
                   out_path: &Path, print_file: &Path, cancel: &Arc<Job>) -> Result<Ran> {
    let mut cmd = proc::command(&runner.program);
    cmd.args(ytdlp::download_args(runner, quality, video_id, out_path, print_file))
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    // Its own tree -- a process group on unix, a Job Object on Windows -- so a
    // cancel reaches the ffmpeg yt-dlp shells out to for the merge, and the
    // real yt-dlp behind PyInstaller's bootloader, as well as the process we
    // spawned. Dropping `child` before it is reaped kills the tree too, which
    // is the backstop for an aborted task.
    let mut child = proc::spawn_tree(&mut cmd)
        .map_err(|e| anyhow!("could not run yt-dlp: {e}"))?;

    // Handing the tree over also re-reads the cancel flag under the one lock,
    // so a cancel landing in this instant cannot miss the process it was meant
    // to kill -- it just leaves the killing to us.
    let tree = child.tree();
    if !cancel.started(tree.clone()) {
        tree.kill();
    }

    let stdout = child.child.stdout.take().ok_or_else(|| anyhow!("no stdout"))?;
    let stderr = child.child.stderr.take().ok_or_else(|| anyhow!("no stderr"))?;

    let app2 = app.clone();
    let vid2 = video_id.to_string();
    let job = cancel.clone();
    let pump = tokio::spawn(async move {
        let emit = |percent, speed, eta| {
            let _ = app2.emit(
                "download://progress",
                DownloadProgress { video_id: vid2.clone(), percent, speed, eta },
            );
        };
        let mut throttle = ProgressThrottle::default();
        let mut held = None;
        let mut lines = BufReader::new(stdout).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            // A cancel has already sent `None` and returned, while yt-dlp is
            // still dying and its pipe still holds lines. A tick after that
            // would put a stopped download back into the views' progress
            // stores, so the rest is read -- the pipe must drain -- and dropped.
            if job.is_cancelled() {
                held = None;
                continue;
            }
            if let Some((percent, speed, eta)) = ytdlp::parse_progress_line(&line) {
                if throttle.admit(Instant::now(), percent) {
                    held = None;
                    emit(percent, speed, eta);
                } else {
                    held = Some((percent, speed, eta));
                }
            }
        }
        // The last line held back, so the bar ends on what yt-dlp last said.
        if let Some((percent, speed, eta)) = held.filter(|_| !job.is_cancelled()) {
            emit(percent, speed, eta);
        }
    });

    let err_collect = tokio::spawn(async move {
        let mut out = String::new();
        let mut lines = BufReader::new(stderr).lines();
        while let Ok(Some(l)) = lines.next_line().await {
            out.push_str(&l);
            out.push('\n');
        }
        out
    });

    let status = child.wait().await?;
    cancel.reaped();
    let _ = pump.await;
    let stderr = err_collect.await.unwrap_or_default();
    Ok(Ran { status, stderr, tree })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A unique `sleep` duration, so `pgrep` finds this test's own children and
    /// nobody else's. It has to stay a number `sleep` accepts, so the process
    /// id goes after the decimal point rather than into a suffix.
    #[cfg(unix)]
    fn probe_seconds() -> String {
        format!("301.{}", std::process::id())
    }

    #[cfg(unix)]
    fn alive(pattern: &str) -> bool {
        let out = std::process::Command::new("pgrep")
            .arg("-f")
            .arg(pattern)
            .output()
            .expect("pgrep is available");
        !out.stdout.is_empty()
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn cancelling_kills_the_grandchild_too_not_just_yt_dlp() {
        // yt-dlp shells out to ffmpeg for the merge. Signalling only the child
        // leaves ffmpeg writing the very file the cancel is about to delete,
        // which is why the download gets a process tree of its own. Driven
        // through a real `Job`, the way `run_one` and `Queue::cancel` do it.
        let secs = probe_seconds();
        let mut cmd = proc::command("sh");
        cmd.arg("-c").arg(format!("sleep {secs} & sleep {secs}"));
        let mut child = proc::spawn_tree(&mut cmd).expect("sh is available");
        let job: Job = Job::default();
        assert!(job.started(child.tree()));

        for _ in 0..40 {
            if alive(&secs) {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        }
        assert!(alive(&secs), "the sleeps should be running before we cancel");

        job.cancel().tree.expect("a running job hands over its tree").kill();
        let _ = child.wait().await;

        for _ in 0..40 {
            if !alive(&secs) {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        // Leave nothing behind for the next run if the assertion is about to fail.
        let _ = std::process::Command::new("pkill").arg("-f").arg(&secs).status();
        panic!("a grandchild outlived the cancel that was supposed to kill it");
    }

    #[test]
    fn a_cancelled_download_sweeps_up_every_shape_yt_dlp_leaves_behind() {
        let out = Path::new("/d/Some Video [2026-01-02].mkv");
        for name in [
            "Some Video [2026-01-02].mkv",                 // the merged file itself
            "Some Video [2026-01-02].mkv.part",            // single-format, in progress
            "Some Video [2026-01-02].f137.mp4",            // a finished stream
            "Some Video [2026-01-02].f251.webm.part",      // a stream in progress
            "Some Video [2026-01-02].f251.webm.ytdl",      // its resume state
            "Some Video [2026-01-02].f137.mp4.part-Frag9", // one DASH fragment
            "Some Video [2026-01-02].temp.mkv",            // ffmpeg's merge target
            "Some Video [2026-01-02].jpg",                 // thumbnail fetched to embed
            "Some Video [2026-01-02].webp",
            "Some Video [2026-01-02].en.srt",              // a subtitle track fetched to embed
            "Some Video [2026-01-02].de-DE.srt",
            "Some Video [2026-01-02].es-419.srt",
            "Some Video [2026-01-02].pt-BR.vtt",           // a track with no srt to offer
            "Some Video [2026-01-02].en-nP7-2PuUl7o.srt",  // a named track, its dot made a hyphen
            "Some Video [2026-01-02].en.srt.part",         // a track in progress
            "Some Video [2026-01-02].meta",                // the chapter list handed to ffmpeg
        ] {
            assert!(is_leftover(out, name), "{name} should be swept up");
        }
    }

    #[test]
    fn a_sidecar_is_a_subtitle_track_only_with_a_language_and_a_subtitle_format() {
        let out = Path::new("/d/Name.mkv");
        for name in [
            "Name.en.mkv",         // a video, whatever its name says
            "Name.en.mp4",
            "Name.en.txt",
            "Name.live_chat.json", // a live replay's chat, never asked for
            "Name..srt",           // no language at all
            "Name.5.srt",          // no language begins with a digit
            "Name.en.us.srt",      // nor holds a dot
            "Name.en.srt.mkv",
        ] {
            assert!(!is_leftover(out, name), "{name} is nothing a download leaves");
        }
    }

    #[test]
    fn a_video_whose_name_extends_another_is_not_swept_up() {
        // "Part 1" and "Part 1.5" are two real uploads, and the second begins
        // with the first's stem followed by a dot. A bare prefix test would
        // delete somebody's downloaded video.
        let out = Path::new("/d/Part 1.mkv");
        for name in [
            "Part 1.5.mkv",
            "Part 1.5.f137.mp4",
            "Part 2.mkv",
            "Part 1 (2).mkv",
            "Part 1.mkv.jpg.mkv",
            "Some other video.mkv",
            "Part 1.5.en.srt",
            "Part 1.5.srt", // `Part 1.5`'s own subtitles; `5` is no language
            "Part 1.5.meta",
        ] {
            assert!(!is_leftover(out, name), "{name} must survive");
        }
    }

    #[test]
    fn before_the_run_without_subtitles_only_the_subtitle_tracks_are_cleared() {
        let out = Path::new("/d/Name.mkv");
        for name in ["Name.en.srt", "Name.de-DE.srt.part", "Name.pt-BR.vtt", "Name.es-419.srt"] {
            assert!(is_subtitle_leftover(out, name), "{name} should go");
        }
        for name in [
            "Name.mkv",
            "Name.mkv.part",
            "Name.f137.mp4.part",
            "Name.temp.mkv",
            "Name.jpg",
            "Name.meta",
            "Name.5.en.srt",
            "Other.en.srt",
        ] {
            assert!(!is_subtitle_leftover(out, name), "{name} must stay");
        }
    }

    #[tokio::test]
    async fn the_subtitle_sweep_clears_the_tracks_and_nothing_else() {
        let dir = std::env::temp_dir().join(format!("mytube-subtitle-sweep-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let out = dir.join("V.mkv");
        for n in ["V.en.srt", "V.de-DE.srt.part", "V.f137.mp4.part", "V.jpg", "W.en.srt"] {
            std::fs::write(dir.join(n), b"x").unwrap();
        }
        sweep_subtitles(vec![out.clone()]).await;
        let mut left: Vec<String> = std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        left.sort();
        assert_eq!(left, vec!["V.f137.mp4.part", "V.jpg", "W.en.srt"]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    const SUBTITLE_429: &str = "[info] Writing video subtitles to: /d/Name.de-DE.srt\n\
        ERROR: Unable to download video subtitles for 'de-DE': HTTP Error 429: Too Many Requests\n";

    #[test]
    fn a_subtitle_track_that_will_not_download_costs_the_subtitles_not_the_video() {
        let subs = Quality { subtitles: true, ..Quality::default() };
        assert!(retry_without_subtitles(&subs, false, false, SUBTITLE_429));
        let mp4 = Quality { container: "mp4".into(), ..subs.clone() };
        assert!(retry_without_subtitles(&mp4, false, false, SUBTITLE_429));

        assert!(!retry_without_subtitles(&subs, true, false, SUBTITLE_429),
                "a cancelled run is the cancel's, not a failure to work around");
        assert!(!retry_without_subtitles(&subs, false, true, SUBTITLE_429),
                "a run that succeeded has nothing to do again");
        assert!(!retry_without_subtitles(&subs, false, false,
                                         "ERROR: [youtube] dQw4w9WgXcQ: Video unavailable\n"),
                "any other failure is the download's own");
    }

    #[test]
    fn the_run_without_subtitles_is_the_last() {
        // Its own failure -- whatever it says -- is final: there is no third run.
        let plain = Quality { subtitles: false, ..Quality::default() };
        assert!(!retry_without_subtitles(&plain, false, false, SUBTITLE_429));
        // Nor is a run that never asked for them retried: an audio file
        // cannot hold subtitles, so yt-dlp was not told to fetch any.
        let audio = Quality { subtitles: true, mode: "audio".into(), audio_format: "mp3".into(),
                              ..Quality::default() };
        assert!(!retry_without_subtitles(&audio, false, false, SUBTITLE_429));
    }

    #[test]
    fn remove_leftovers_clears_the_job_and_leaves_the_neighbours_alone() {
        let dir = std::env::temp_dir().join("mytube-leftover-sweep-test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let out = dir.join("Part 1.mkv");
        for n in ["Part 1.mkv", "Part 1.f137.mp4.part", "Part 1.f251.webm", "Part 1.jpg"] {
            std::fs::write(dir.join(n), b"x").unwrap();
        }
        for n in ["Part 1.5.mkv", "Part 2.mkv"] {
            std::fs::write(dir.join(n), b"x").unwrap();
        }

        assert!(remove_leftovers(&out, is_leftover).is_empty(), "nothing is held open here");

        let mut left: Vec<String> = std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        left.sort();
        assert_eq!(left, vec!["Part 1.5.mkv".to_string(), "Part 2.mkv".to_string()]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn the_async_sweep_clears_the_same_files() {
        let dir = std::env::temp_dir().join(format!("mytube-async-sweep-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let out = dir.join("V.mkv");
        for n in ["V.mkv.part", "V.f137.mp4", "W.mkv"] {
            std::fs::write(dir.join(n), b"x").unwrap();
        }
        sweep_leftovers(vec![out.clone()]).await;
        let left: Vec<String> = std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(left, vec!["W.mkv".to_string()]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The Windows failure this retry exists for: a part file still open in a
    /// process that is on its way out. Opened here with no sharing at all --
    /// std's default would allow the delete -- and let go of 500 ms later, the
    /// way a dying ffmpeg lets go of the stream it was writing.
    #[cfg(windows)]
    #[tokio::test]
    async fn a_part_file_still_held_open_is_deleted_once_it_is_let_go() {
        use std::os::windows::fs::OpenOptionsExt;
        let dir = std::env::temp_dir().join(format!("mytube-held-sweep-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let out = dir.join("V.mkv");
        let part = dir.join("V.f251.webm.part");
        std::fs::write(&part, b"x").unwrap();
        let held = std::fs::OpenOptions::new().read(true).share_mode(0).open(&part).unwrap();

        assert_eq!(remove_leftovers(&out, is_leftover), vec![part.clone()],
                   "held open, so it cannot go yet");
        let release = std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(500));
            drop(held);
        });
        sweep_leftovers(vec![out.clone()]).await;
        release.join().unwrap();
        assert!(!part.exists(), "the retry should have deleted it once it was closed");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// `Rows` in memory: each row's state, and every event in the order sent.
    #[derive(Default)]
    struct FakeRows {
        rows: std::sync::Mutex<HashMap<String, DownloadState>>,
        sent: std::sync::Mutex<Vec<DownloadState>>,
    }
    impl Rows for FakeRows {
        fn set(&self, id: &str, state: DownloadState) -> Result<()> {
            self.rows.lock().unwrap().insert(id.to_string(), state);
            self.sent.lock().unwrap().push(state);
            Ok(())
        }
        fn state(&self, id: &str) -> Option<DownloadState> {
            self.rows.lock().unwrap().get(id).copied()
        }
    }

    /// Waits, at most two seconds, for `f` to hold.
    async fn eventually(f: impl Fn() -> bool) -> bool {
        for _ in 0..200 {
            if f() {
                return true;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        f()
    }

    /// A registry letting `slots` downloads run at once.
    fn registry(slots: usize) -> Arc<Registry<i32>> {
        Registry::new(Arc::new(Semaphore::new(slots)))
    }

    /// Waits for the cancelled download of `id` to finish clearing up.
    async fn cleared(reg: &Registry<i32>, id: &str) {
        let pending = reg.book().cleaning.get(id).cloned();
        if let Some(g) = pending {
            tokio::time::timeout(Duration::from_secs(5), g.wait()).await
                .expect("the cancelled download should clear up well inside the grace");
        }
    }

    /// A job body that plays a running download: yt-dlp "spawned" at once,
    /// then, once cancelled, `sweep` spent deleting part files before `done`.
    fn running_download(
        ready: tokio::sync::oneshot::Sender<()>,
        sweep: Duration,
        done: impl FnOnce() + Send + 'static,
    ) -> impl FnOnce(Arc<Job<i32>>, Alive) -> std::pin::Pin<Box<dyn Future<Output = ()> + Send>> {
        move |job, _alive| {
            Box::pin(async move {
                assert!(job.started(4242));
                let _ = ready.send(());
                while !job.is_cancelled() {
                    tokio::time::sleep(Duration::from_millis(5)).await;
                }
                tokio::time::sleep(sweep).await;
                done();
            })
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn cancelling_a_running_download_returns_at_once_and_clears_up_behind() {
        // The deadlock this replaces: cancel held the `running` map's lock
        // while it waited for the job, and the job's last act was to take that
        // lock -- so the cancel button hung for the full 15 s grace. Now it
        // must not wait for the job at all, not even for a sweep that works.
        let reg = registry(4);
        let rows = Arc::new(FakeRows::default());
        let swept = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let (tx, rx) = tokio::sync::oneshot::channel();
        let s = swept.clone();
        reg.enqueue("v".into(), &rows,
                    running_download(tx, Duration::from_millis(300),
                                     move || s.store(true, std::sync::atomic::Ordering::SeqCst)))
            .unwrap();
        rx.await.unwrap();

        let t = Instant::now();
        reg.cancel("v", &rows).unwrap();
        assert!(t.elapsed() < Duration::from_millis(150),
                "cancel took {:?}; it must not wait for the job", t.elapsed());
        assert_eq!(rows.state("v"), Some(DownloadState::None), "the row reads None at once");
        assert!(!reg.is_active("v"));
        assert!(!swept.load(std::sync::atomic::Ordering::SeqCst), "the sweep is still going");

        cleared(&reg, "v").await;
        assert!(swept.load(std::sync::atomic::Ordering::SeqCst));
        assert!(reg.book().cleaning.is_empty(), "the record goes once the sweep is done");
        assert_eq!(*rows.sent.lock().unwrap(), vec![DownloadState::Queued, DownloadState::None],
                   "nothing moved the row after the cancel, so nothing is sent twice");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_download_restarted_after_a_cancel_waits_for_the_old_one_to_clear_up() {
        // The old sweep deletes by the old job's names -- the same print file,
        // the same claimed path, the same `.part` files -- so the new download
        // must not start until it is over. And the old job's exit must not
        // take the new job's entry with it.
        let reg = registry(4);
        let rows = Arc::new(FakeRows::default());
        let log = Arc::new(std::sync::Mutex::new(Vec::<&'static str>::new()));
        let (tx, rx) = tokio::sync::oneshot::channel();
        let l = log.clone();
        reg.enqueue("v".into(), &rows,
                    running_download(tx, Duration::from_millis(200),
                                     move || l.lock().unwrap().push("old swept")))
            .unwrap();
        rx.await.unwrap();
        reg.cancel("v", &rows).unwrap();

        let (release_tx, release_rx) = tokio::sync::oneshot::channel::<()>();
        let l = log.clone();
        reg.enqueue("v".into(), &rows, move |_job, _alive| async move {
            l.lock().unwrap().push("new started");
            let _ = release_rx.await;
        })
        .unwrap();
        assert!(reg.is_active("v"));

        assert!(eventually(|| log.lock().unwrap().contains(&"new started")).await);
        assert_eq!(*log.lock().unwrap(), vec!["old swept", "new started"]);
        cleared(&reg, "v").await;
        assert!(reg.is_active("v"), "the old job's exit removed the new job's entry");
        assert_eq!(rows.state("v"), Some(DownloadState::Queued),
                   "the old cancel's clean-up must not overwrite the new download's row");

        release_tx.send(()).unwrap();
        assert!(eventually(|| !reg.is_active("v")).await, "the new job's own exit clears it");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_write_landing_after_the_cancel_is_put_back_to_none() {
        // A `Done` that slips in after the cancel's `None`, while the file it
        // names is being deleted, must not be where the row ends up.
        let reg = registry(4);
        let rows = Arc::new(FakeRows::default());
        let (tx, rx) = tokio::sync::oneshot::channel();
        let r = rows.clone();
        reg.enqueue("v".into(), &rows,
                    running_download(tx, Duration::ZERO,
                                     move || { let _ = r.set("v", DownloadState::Done); }))
            .unwrap();
        rx.await.unwrap();
        reg.cancel("v", &rows).unwrap();
        cleared(&reg, "v").await;
        assert_eq!(rows.state("v"), Some(DownloadState::None));
        assert_eq!(rows.sent.lock().unwrap().last(), Some(&DownloadState::None),
                   "and the UI is told so");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_queued_download_is_aborted_where_it_stands() {
        // Parked on the semaphore behind other downloads, with nothing on
        // disk: waiting for it to notice the cancel could take minutes.
        struct Dropped(Arc<std::sync::atomic::AtomicBool>);
        impl Drop for Dropped {
            fn drop(&mut self) {
                self.0.store(true, std::sync::atomic::Ordering::SeqCst);
            }
        }
        let reg = registry(4);
        let rows = Arc::new(FakeRows::default());
        let dropped = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let d = Dropped(dropped.clone());
        reg.enqueue("v".into(), &rows, move |_job, _alive| async move {
            let _d = d;
            std::future::pending::<()>().await;
        })
        .unwrap();
        tokio::time::sleep(Duration::from_millis(20)).await;
        reg.cancel("v", &rows).unwrap();
        cleared(&reg, "v").await;
        assert!(dropped.load(std::sync::atomic::Ordering::SeqCst), "the task was aborted");
        assert!(!reg.is_active("v"));
        assert_eq!(rows.state("v"), Some(DownloadState::None));
    }

    #[tokio::test]
    async fn an_aborted_job_drops_the_live_run_before_its_cleanup_guard() {
        // The shape `run_one` relies on: `CleanupGuard` is a local, and the
        // child lives inside the `run_ytdlp` future awaited after it. Aborted
        // mid-run, that future must go first -- killing yt-dlp's tree -- and
        // only then the guard, whose sweep would otherwise race a live yt-dlp.
        type Log = Arc<std::sync::Mutex<Vec<&'static str>>>;
        struct Mark(&'static str, Log);
        impl Drop for Mark {
            fn drop(&mut self) {
                self.1.lock().unwrap().push(self.0);
            }
        }
        async fn run(log: Log) {
            let _child = Mark("child", log);
            std::future::pending::<()>().await;
        }
        let log: Log = Default::default();
        let l = log.clone();
        let task = tokio::spawn(async move {
            let _cleanup = Mark("cleanup", l.clone());
            run(l).await;
        });
        tokio::time::sleep(Duration::from_millis(20)).await;
        task.abort();
        let _ = task.await;
        assert_eq!(*log.lock().unwrap(), vec!["child", "cleanup"]);
    }

    #[tokio::test]
    async fn enqueueing_a_download_already_in_flight_does_nothing() {
        let reg = registry(4);
        let rows = Arc::new(FakeRows::default());
        let (tx, rx) = tokio::sync::oneshot::channel::<()>();
        reg.enqueue("v".into(), &rows, move |_j, _a| async move { let _ = rx.await; }).unwrap();
        reg.enqueue("v".into(), &rows, |_j, _a| async { panic!("a second job was started") })
            .unwrap();
        assert_eq!(*rows.sent.lock().unwrap(), vec![DownloadState::Queued]);
        tx.send(()).unwrap();
        assert!(eventually(|| !reg.is_active("v")).await);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn places_are_served_in_the_order_handed_out_not_the_order_they_arrive() {
        // The race the line exists for, made certain rather than likely: each
        // later place dawdles *less* before asking, so without the line they
        // would reach the semaphore last-queued first. One slot, held a moment
        // by each, so the log is the order the slot was handed out in.
        let line = Line::new();
        let slots = Arc::new(Semaphore::new(1));
        let log = Arc::new(std::sync::Mutex::new(Vec::new()));
        let n = 24u64;
        let mut tasks = Vec::new();
        for i in 0..n {
            let place = line.join();
            let (slots, log) = (slots.clone(), log.clone());
            tasks.push(tokio::spawn(async move {
                tokio::time::sleep(Duration::from_millis(n - i)).await;
                let _slot = place.admit(&slots).await.expect("a slot");
                log.lock().unwrap().push(i);
                tokio::time::sleep(Duration::from_millis(1)).await;
            }));
        }
        for t in tasks {
            t.await.unwrap();
        }
        assert_eq!(*log.lock().unwrap(), (0..n).collect::<Vec<_>>());
    }

    #[tokio::test]
    async fn a_place_left_before_its_turn_is_skipped_and_forgotten() {
        let line = Line::new();
        let slots = Arc::new(Semaphore::new(1));
        let (a, b, c) = (line.join(), line.join(), line.join());
        drop(b); // a cancel, while a still had the front
        drop(a.admit(&slots).await.expect("a slot"));
        let c = tokio::time::timeout(Duration::from_secs(1), c.admit(&slots)).await
            .expect("c waited for a turn nobody was going to take");
        drop(c.expect("a slot"));
        assert!(line.turns().left.is_empty(), "b's number was skipped, not kept forever");
        // And the line, empty now, serves whoever comes next at once.
        let next = tokio::time::timeout(Duration::from_secs(1), line.join().admit(&slots)).await
            .expect("an empty line kept a newcomer waiting");
        assert!(next.is_some());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn leaving_out_of_turn_never_lets_the_next_place_overtake() {
        // b gives up while a, ahead of it, has not been served. c is behind
        // b, but it must not take that as its cue: a is still waiting. Slots
        // to spare, so only the line can be holding c back.
        let line = Line::new();
        let slots = Arc::new(Semaphore::new(8));
        let (a, b, c) = (line.join(), line.join(), line.join());
        let s = slots.clone();
        let c = tokio::spawn(async move { c.admit(&s).await.map(drop) });
        drop(b);
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(!c.is_finished(), "c overtook a");
        let _a = a.admit(&slots).await.expect("a slot");
        tokio::time::timeout(Duration::from_secs(1), c).await
            .expect("c was not served once a had been")
            .unwrap()
            .expect("a slot");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn the_front_of_the_line_given_up_while_waiting_for_a_slot_hands_its_turn_on() {
        // a is at the front and parked on the semaphore, every slot taken,
        // when its task is aborted -- a cancel of the next download due.
        let line = Line::new();
        let slots = Arc::new(Semaphore::new(1));
        let busy = slots.clone().acquire_owned().await.unwrap();
        let (a, b) = (line.join(), line.join());
        let s = slots.clone();
        let a = tokio::spawn(async move { a.admit(&s).await.map(drop) });
        let s = slots.clone();
        let b = tokio::spawn(async move { b.admit(&s).await.map(drop) });
        tokio::time::sleep(Duration::from_millis(20)).await;
        a.abort();
        let _ = a.await;
        drop(busy);
        tokio::time::timeout(Duration::from_secs(1), b).await
            .expect("b waited on a download that is gone")
            .unwrap()
            .expect("a slot");
    }

    /// Ids in the order their fake downloads began.
    type Starts = Arc<std::sync::Mutex<Vec<String>>>;

    /// A fake download that notes its start in `starts` and ends at once.
    fn quick(starts: &Starts, id: &str)
             -> impl FnOnce(Arc<Job<i32>>, Alive) -> std::pin::Pin<Box<dyn Future<Output = ()> + Send>> {
        let (starts, id) = (starts.clone(), id.to_string());
        move |_job, _alive| Box::pin(async move { starts.lock().unwrap().push(id) })
    }

    /// A fake download that notes its start and holds its slot until `go`
    /// fires or is dropped.
    fn held(starts: &Starts, id: &str, go: tokio::sync::oneshot::Receiver<()>)
            -> impl FnOnce(Arc<Job<i32>>, Alive) -> std::pin::Pin<Box<dyn Future<Output = ()> + Send>> {
        let (starts, id) = (starts.clone(), id.to_string());
        move |_job, _alive| Box::pin(async move {
            starts.lock().unwrap().push(id);
            let _ = go.await;
        })
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn downloads_start_in_the_order_they_were_queued() {
        // "Download all" on a series: part 1 first. One slot, each part
        // holding it a different while, through the real `enqueue`.
        let reg = registry(1);
        let rows = Arc::new(FakeRows::default());
        let starts: Starts = Default::default();
        let ids: Vec<String> = (0..16).map(|i| format!("part {i:02}")).collect();
        for (i, id) in ids.iter().enumerate() {
            let (s, me) = (starts.clone(), id.clone());
            reg.enqueue(id.clone(), &rows, move |_job, _alive| async move {
                s.lock().unwrap().push(me);
                tokio::time::sleep(Duration::from_millis(i as u64 % 3)).await;
            })
            .unwrap();
        }
        assert!(eventually(|| starts.lock().unwrap().len() == ids.len()).await);
        assert_eq!(*starts.lock().unwrap(), ids);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_download_cancelled_mid_line_holds_up_nobody_and_lets_nobody_jump() {
        // b is at the front, parked on the one slot a is using; c behind it,
        // d behind that. Cancelling c must neither stall d for good nor let it
        // overtake b.
        let reg = registry(1);
        let rows = Arc::new(FakeRows::default());
        let starts: Starts = Default::default();
        let (go, wait) = tokio::sync::oneshot::channel();
        reg.enqueue("a".into(), &rows, held(&starts, "a", wait)).unwrap();
        for id in ["b", "c", "d"] {
            reg.enqueue(id.into(), &rows, quick(&starts, id)).unwrap();
        }
        assert!(eventually(|| starts.lock().unwrap().len() == 1).await);
        reg.cancel("c", &rows).unwrap();
        cleared(&reg, "c").await;
        go.send(()).unwrap();
        assert!(eventually(|| starts.lock().unwrap().len() == 3).await, "d never started");
        assert_eq!(*starts.lock().unwrap(), ["a", "b", "d"]);
        assert_eq!(rows.state("c"), Some(DownloadState::None));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_download_cancelled_at_the_front_hands_its_turn_on() {
        // b is the one download waiting on the semaphore itself.
        let reg = registry(1);
        let rows = Arc::new(FakeRows::default());
        let starts: Starts = Default::default();
        let (go, wait) = tokio::sync::oneshot::channel();
        reg.enqueue("a".into(), &rows, held(&starts, "a", wait)).unwrap();
        for id in ["b", "c"] {
            reg.enqueue(id.into(), &rows, quick(&starts, id)).unwrap();
        }
        assert!(eventually(|| starts.lock().unwrap().len() == 1).await);
        reg.cancel("b", &rows).unwrap();
        cleared(&reg, "b").await;
        go.send(()).unwrap();
        assert!(eventually(|| starts.lock().unwrap().len() == 2).await, "c never started");
        assert_eq!(*starts.lock().unwrap(), ["a", "c"]);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_download_that_panics_gives_up_its_slot() {
        // A panic unwinds through the task, slot and all; the next download
        // must not wait on a permit the panic took with it.
        let reg = registry(1);
        let rows = Arc::new(FakeRows::default());
        let starts: Starts = Default::default();
        reg.enqueue("a".into(), &rows, |_j, _a| async { panic!("a download blew up") }).unwrap();
        reg.enqueue("b".into(), &rows, quick(&starts, "b")).unwrap();
        assert!(eventually(|| starts.lock().unwrap().len() == 1).await, "b never started");
        assert!(eventually(|| !reg.is_active("a")).await);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn new_slots_go_to_the_downloads_queued_first() {
        // Raising the limit mid-queue: the two new slots go to b and c, the
        // two waiting longest, and d still waits.
        let reg = registry(1);
        let rows = Arc::new(FakeRows::default());
        let starts: Starts = Default::default();
        let mut go = Vec::new();
        for id in ["a", "b", "c", "d"] {
            let (tx, rx) = tokio::sync::oneshot::channel();
            go.push(tx);
            reg.enqueue(id.into(), &rows, held(&starts, id, rx)).unwrap();
        }
        assert!(eventually(|| starts.lock().unwrap().len() == 1).await);
        reg.slots.add_permits(2); // what `Queue::set_concurrency` does
        assert!(eventually(|| starts.lock().unwrap().len() == 3).await);
        tokio::time::sleep(Duration::from_millis(50)).await;
        let mut started = starts.lock().unwrap().clone();
        started.sort(); // b and c start side by side, in either order
        assert_eq!(started, ["a", "b", "c"]);
        drop(go);
        assert!(eventually(|| starts.lock().unwrap().len() == 4).await);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_restarted_download_waits_for_its_clean_up_without_holding_up_the_line() {
        // x is cancelled mid-download and queued again at once, y behind it.
        // The new x must wait for the old one's sweep; y, with a slot free,
        // must not wait for it too.
        let reg = registry(3);
        let rows = Arc::new(FakeRows::default());
        let log = Arc::new(std::sync::Mutex::new(Vec::<&'static str>::new()));
        let (tx, rx) = tokio::sync::oneshot::channel();
        let l = log.clone();
        reg.enqueue("x".into(), &rows,
                    running_download(tx, Duration::from_millis(500),
                                     move || l.lock().unwrap().push("old x swept")))
            .unwrap();
        rx.await.unwrap();
        reg.cancel("x", &rows).unwrap();
        for (id, mark) in [("x", "new x started"), ("y", "y started")] {
            let l = log.clone();
            reg.enqueue(id.into(), &rows, move |_j, _a| async move {
                l.lock().unwrap().push(mark);
            })
            .unwrap();
        }
        assert!(eventually(|| log.lock().unwrap().len() == 3).await);
        assert_eq!(*log.lock().unwrap(), ["y started", "old x swept", "new x started"]);
    }

    #[test]
    fn a_cancelled_job_does_not_write_its_row() {
        // `Done` and `Failed` go through `unless_cancelled`, so a cancel that
        // got in first leaves the row to the cancel.
        let job = Job::<i32>::default();
        assert_eq!(job.unless_cancelled(|| 1), Some(1));
        job.cancel();
        assert_eq!(job.unless_cancelled(|| panic!("wrote after a cancel")), None::<()>);
    }

    #[test]
    fn progress_is_thinned_but_never_left_stale() {
        let mut t = ProgressThrottle::default();
        let t0 = Instant::now();
        let at = |ms| t0 + Duration::from_millis(ms);
        assert!(t.admit(at(0), 0.0), "the first line always goes");
        assert!(!t.admit(at(50), 1.0));
        assert!(!t.admit(at(200), 2.0));
        assert!(t.admit(at(260), 3.0), "a quarter of a second on, the next one goes");
        assert!(!t.admit(at(300), 4.0));
        assert!(t.admit(at(310), 100.0), "100% always goes");
        assert!(t.admit(at(320), 0.2), "so does the second stream starting from 0%");
        assert!(!t.admit(at(330), 0.4));
    }

    #[test]
    fn a_job_cancelled_before_yt_dlp_starts_has_nothing_to_kill_and_nothing_to_wait_for() {
        let job = Job::<i32>::default();
        let stop = job.cancel();
        assert_eq!(stop.tree, None);
        assert!(!stop.spawned, "nothing was spawned, so there is no part file to clear");
        assert!(job.is_cancelled());
    }

    #[test]
    fn a_cancel_racing_the_spawn_is_not_lost() {
        // The cancel lands between "am I cancelled?" and "here is my yt-dlp".
        // `started` must refuse the handover so the caller kills what it just
        // spawned -- otherwise the download runs on with nothing to stop it.
        let job = Job::<i32>::default();
        job.cancel();
        assert!(!job.started(4242), "a cancelled job must not adopt a tree");
    }

    #[test]
    fn cancelling_a_running_job_yields_the_group_to_kill_exactly_once() {
        let job = Job::<i32>::default();
        assert!(job.started(4242));
        let stop = job.cancel();
        assert_eq!(stop.tree, Some(4242));
        assert!(stop.spawned);
        // A second cancel must not signal a pid that has since been reused.
        assert_eq!(job.cancel().tree, None);
    }

    #[test]
    fn a_reaped_child_still_counts_as_spawned() {
        // yt-dlp has exited but `run_one` is still writing the row. There is no
        // group left to kill, yet a file is on disk -- so `cancel` must still
        // wait for the job to clear up rather than abort it where it stands.
        let job = Job::<i32>::default();
        assert!(job.started(4242));
        job.reaped();
        let stop = job.cancel();
        assert_eq!(stop.tree, None);
        assert!(stop.spawned);
    }

    #[test]
    fn the_run_without_subtitles_is_adopted_once_the_first_is_reaped() {
        // One job, two yt-dlps one after the other: a cancel must reach the
        // one that is running, never the pid of the one already reaped.
        let job = Job::<i32>::default();
        assert!(job.started(1));
        job.reaped();
        assert!(job.started(2), "the second run is the job's as much as the first");
        let stop = job.cancel();
        assert_eq!(stop.tree, Some(2));
        assert!(stop.spawned);
    }

    #[test]
    fn a_cancel_between_the_two_runs_stops_the_second_before_it_starts() {
        // It lands while the first run's subtitle tracks are being deleted.
        // Nothing is running to kill, but files are on disk, so the cancel
        // waits for the job's sweep -- and the job must not adopt the yt-dlp
        // it spawns next, which leaves the kill to `run_ytdlp`.
        let job = Job::<i32>::default();
        assert!(job.started(1));
        job.reaped();
        let stop = job.cancel();
        assert_eq!(stop.tree, None);
        assert!(stop.spawned);
        assert!(!job.started(2));
    }

    #[test]
    fn stderr_is_truncated_to_the_last_2000_chars() {
        let long = "x".repeat(5000);
        let t = tail(&long, 2000);
        assert_eq!(t.len(), 2000);
        let short = "boom";
        assert_eq!(tail(short, 2000), "boom");
    }

    #[test]
    fn tail_keeps_the_end_not_the_start() {
        let s = format!("{}ERROR_AT_END", "x".repeat(3000));
        assert!(tail(&s, 100).ends_with("ERROR_AT_END"));
    }

    #[test]
    fn tail_does_not_split_a_multibyte_character() {
        let s = "é".repeat(2000);
        let t = tail(&s, 101);
        assert!(t.chars().count() <= 101);
        assert!(std::str::from_utf8(t.as_bytes()).is_ok());
    }

    #[test]
    fn claims_make_a_path_taken_before_the_file_exists() {
        let claims = PathClaims::default();
        let p = PathBuf::from("/out/Video.mkv");
        assert!(!claims.is_claimed(&p));
        assert!(claims.claim(p.clone()), "first claim succeeds");
        assert!(claims.is_claimed(&p));
        assert!(!claims.claim(p.clone()), "second claim on the same path fails");
        claims.release(&p);
        assert!(!claims.is_claimed(&p));
    }

    #[test]
    fn two_same_named_downloads_get_different_paths() {
        let claims = PathClaims::default();
        let intended = PathBuf::from("/out/Same Title.mkv");
        // Nothing on disk; only claims decide.
        let first = claims.claim_unique(&intended, |_| false);
        let second = claims.claim_unique(&intended, |_| false);
        assert_eq!(first, PathBuf::from("/out/Same Title.mkv"));
        assert_eq!(second, PathBuf::from("/out/Same Title (2).mkv"));
        assert_ne!(first, second);
    }

    /// Downloads a short video once per kind of quality, through the same
    /// steps `run_one` takes -- probe, `intended_path`, claim, `download_args`,
    /// read back the printed path -- and checks what lands on disk:
    ///
    ///     cargo test --manifest-path src-tauri/Cargo.toml real_download_in_each_mode -- --ignored --nocapture
    #[tokio::test]
    #[ignore = "requires network, yt-dlp, ffmpeg and deno"]
    async fn real_download_in_each_mode() {
        const VIDEO: &str = "jNQXAC9IVRw"; // "Me at the zoo", 19 s
        let settings = config::load().unwrap_or_default();
        let tools = Tools::new(crate::tools::bin_dir(), reqwest::Client::new());
        let inv = tools.ytdlp(&settings).await.expect("a yt-dlp");
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().to_string_lossy().into_owned();
        let claims = PathClaims::default();

        let audio = |f: &str| Quality { mode: "audio".into(), audio_format: f.into(),
                                        ..Quality::default() };
        let cases: Vec<(&str, Quality, Option<&str>)> = vec![
            ("default", Quality::default(), Some("mkv")),
            ("mp4 at 360p", Quality { container: "mp4".into(), max_height: 360,
                                     ..Quality::default() }, Some("mp4")),
            ("audio mp3", audio("mp3"), Some("mp3")),
            ("audio opus", audio("opus"), Some("opus")),
            ("audio original", audio("original"), None),
        ];
        for (name, q, want_ext) in cases {
            let probe = ytdlp::probe(&inv, &q, &ytdlp::watch_url(VIDEO), &dir,
                                     &format!("{name} [%(id)s].%(ext)s"))
                .await.unwrap_or_else(|e| panic!("{name}: probe failed: {e}"));
            let probed = PathBuf::from(&probe.intended_path);
            let (intended, via) = intended_path(&probed, quality::final_ext(&q));
            let (out, _) = claims.claim_unique_with(&intended, via.as_deref(), |c| c.exists());
            let print_file = tmp.path().join(format!("{name}.path"));

            let out_status = proc::command(&inv.runner.program)
                .args(ytdlp::download_args(&inv.runner, &q, VIDEO, &out, &print_file))
                .output().await.expect("yt-dlp runs");
            let stderr = String::from_utf8_lossy(&out_status.stderr);
            assert!(out_status.status.success(), "{name}: yt-dlp failed: {stderr}");

            let landed = PathBuf::from(std::fs::read_to_string(&print_file).unwrap().trim());
            let ext = landed.extension().unwrap().to_string_lossy().into_owned();
            println!("{name}: probe said {}, claimed {}, landed {} ({} bytes)",
                     probed.file_name().unwrap().to_string_lossy(),
                     out.file_name().unwrap().to_string_lossy(),
                     landed.file_name().unwrap().to_string_lossy(),
                     std::fs::metadata(&landed).map(|m| m.len()).unwrap_or(0));
            assert!(landed.exists(), "{name}: {landed:?} is missing");
            assert_eq!(landed, out, "{name}: the file did not land where it was claimed");
            match want_ext {
                Some(e) => assert_eq!(ext, e, "{name}"),
                None => assert_eq!(Some(ext.as_str()),
                                   probed.extension().and_then(|e| e.to_str()),
                                   "{name}: original audio keeps the probe's extension"),
            }
        }
        let mut left: Vec<String> = std::fs::read_dir(tmp.path()).unwrap()
            .filter_map(|e| e.ok()).map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| !n.ends_with(".path"))
            .collect();
        left.sort();
        println!("directory: {left:?}");
    }

    /// Downloads with subtitles and with chapters through the same steps as
    /// `real_download_in_each_mode`, asks ffprobe what landed, and checks that
    /// nothing was left beside it -- every subtitle track and chapter list
    /// yt-dlp wrote on the way embedded and deleted:
    ///
    ///     cargo test --manifest-path src-tauri/Cargo.toml real_download_embeds_subtitles_and_chapters -- --ignored --nocapture
    ///
    /// It does not go through `run_one`, so a subtitle track YouTube refuses
    /// (HTTP 429) fails it rather than falling back: run it again later.
    #[tokio::test]
    #[ignore = "requires network, yt-dlp, ffmpeg and deno"]
    async fn real_download_embeds_subtitles_and_chapters() {
        const SUBTITLED: &str = "dQw4w9WgXcQ"; // 5 uploader tracks, no chapters
        const CHAPTERED: &str = "cko1HzN9OA4"; // 7 chapters, no uploader tracks
        let settings = config::load().unwrap_or_default();
        let tools = Tools::new(crate::tools::bin_dir(), reqwest::Client::new());
        let inv = tools.ytdlp(&settings).await.expect("a yt-dlp");
        let ffprobe_exe = if cfg!(windows) { "ffprobe.exe" } else { "ffprobe" };
        let ffprobe = match &inv.runner.ffmpeg_location {
            Some(loc) if loc.is_dir() => loc.join(ffprobe_exe),
            _ => PathBuf::from(ffprobe_exe),
        };
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().to_string_lossy().into_owned();
        let claims = PathClaims::default();

        // What ffprobe says of `file` when asked `args`: a line per stream or
        // chapter it picks out.
        let probe = |file: PathBuf, args: &'static [&'static str]| {
            let ffprobe = ffprobe.clone();
            async move {
                let out = proc::command(&ffprobe).args(["-v", "error"]).args(args)
                    .args(["-of", "csv=p=0"]).arg(&file)
                    .output().await.expect("ffprobe runs");
                assert!(out.status.success(), "ffprobe: {}", String::from_utf8_lossy(&out.stderr));
                String::from_utf8_lossy(&out.stdout).lines()
                    .filter(|l| !l.trim().is_empty()).map(str::to_string).collect::<Vec<_>>()
            }
        };

        let small = Quality { max_height: 144, ..Quality::default() };
        let cases = [
            ("subtitles mkv", SUBTITLED, Quality { subtitles: true, ..small.clone() }),
            ("subtitles mp4", SUBTITLED,
             Quality { subtitles: true, container: "mp4".into(), ..small.clone() }),
            ("chapters", CHAPTERED, small.clone()),
        ];
        let mut landed_names = Vec::new();
        for (name, video, q) in cases {
            let probed = ytdlp::probe(&inv, &q, &ytdlp::watch_url(video), &dir,
                                      &format!("{name} [%(id)s].%(ext)s"))
                .await.unwrap_or_else(|e| panic!("{name}: probe failed: {e}"));
            let probed = PathBuf::from(&probed.intended_path);
            let (intended, via) = intended_path(&probed, quality::final_ext(&q));
            let (out, _) = claims.claim_unique_with(&intended, via.as_deref(), |c| c.exists());
            let print_file = tmp.path().join(format!("{name}.path"));

            let args = ytdlp::download_args(&inv.runner, &q, video, &out, &print_file);
            let out_status = proc::command(&inv.runner.program).args(&args)
                .output().await.expect("yt-dlp runs");
            let stderr = String::from_utf8_lossy(&out_status.stderr);
            assert!(out_status.status.success(), "{name}: yt-dlp failed: {stderr}");

            let landed = PathBuf::from(std::fs::read_to_string(&print_file).unwrap().trim());
            assert_eq!(landed, out, "{name}: the file did not land where it was claimed");
            const SUBTITLE_STREAMS: &[&str] =
                &["-select_streams", "s", "-show_entries", "stream=codec_name:stream_tags=language"];
            let subs = probe(landed.clone(), SUBTITLE_STREAMS).await;
            let chapters = probe(landed.clone(), &["-show_entries", "chapter=start_time"]).await;
            println!("{name}: {} -- {} subtitle streams {subs:?}, {} chapters",
                     landed.file_name().unwrap().to_string_lossy(), subs.len(), chapters.len());
            if q.subtitles {
                assert!(!subs.is_empty(), "{name}: no subtitle stream was embedded");
            }
            if video == CHAPTERED {
                assert!(!chapters.is_empty(), "{name}: no chapter was embedded");
            }
            landed_names.push(landed.file_name().unwrap().to_string_lossy().into_owned());
        }

        let mut left: Vec<String> = std::fs::read_dir(tmp.path()).unwrap()
            .filter_map(|e| e.ok()).map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| !n.ends_with(".path"))
            .collect();
        left.sort();
        landed_names.sort();
        assert_eq!(left, landed_names, "a sidecar was left beside the finished files");
    }

    #[test]
    fn the_intended_path_takes_the_extension_post_processing_gives_it() {
        let p = Path::new("/out/Ch/Title [id].m4a");
        assert_eq!(intended_path(p, Some("mp3")),
                   (PathBuf::from("/out/Ch/Title [id].mp3"), Some("m4a".to_string())));
        assert_eq!(intended_path(p, None), (p.to_path_buf(), None), "original audio keeps it");
        let mkv = Path::new("/out/S01.E02 Pilot [id].mkv");
        assert_eq!(intended_path(mkv, Some("mkv")), (mkv.to_path_buf(), None),
                   "a merge the probe already named needs nothing");
        assert_eq!(intended_path(Path::new("/out/1.5 [id].mp4"), Some("mkv")).0,
                   PathBuf::from("/out/1.5 [id].mkv"), "only the last dot is the extension");
    }

    #[test]
    fn a_conversion_claims_its_intermediate_name_too() {
        let claims = PathClaims::default();
        let (out, via) = claims.claim_unique_with(Path::new("/o/T.mp3"), Some("m4a"), |_| false);
        assert_eq!(out, PathBuf::from("/o/T.mp3"));
        assert_eq!(via, Some(PathBuf::from("/o/T.m4a")));
        assert!(claims.is_claimed(Path::new("/o/T.m4a")));
        // An original-audio download of a same-named video now cannot take
        // the name the conversion is passing through.
        assert_eq!(claims.claim_unique(Path::new("/o/T.m4a"), |_| false),
                   PathBuf::from("/o/T (2).m4a"));
    }

    #[test]
    fn an_intermediate_name_already_on_disk_moves_the_whole_pair() {
        let claims = PathClaims::default();
        let on_disk = |c: &Path| c == Path::new("/o/T.m4a");
        let (out, via) = claims.claim_unique_with(Path::new("/o/T.mp3"), Some("m4a"), on_disk);
        assert_eq!(out, PathBuf::from("/o/T (2).mp3"));
        assert_eq!(via, Some(PathBuf::from("/o/T (2).m4a")));
    }

    #[test]
    fn a_file_already_on_disk_is_also_taken() {
        let claims = PathClaims::default();
        let intended = PathBuf::from("/out/Exists.mkv");
        let on_disk = |c: &Path| c == Path::new("/out/Exists.mkv");
        assert_eq!(
            claims.claim_unique(&intended, on_disk),
            PathBuf::from("/out/Exists (2).mkv")
        );
    }
}
