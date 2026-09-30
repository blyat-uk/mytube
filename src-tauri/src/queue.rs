use anyhow::{anyhow, Result};
use std::collections::HashMap;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::sync::{watch, Mutex, Semaphore};

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
    /// The tree yt-dlp is running in, for as long as it is running.
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
    /// itself -- nobody else now holds a handle to it.
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

/// Does `name` look like something yt-dlp wrote on the way to `out_path`?
///
/// Matched by shape rather than by a bare "starts with the stem" test, because
/// two real uploads can be `Part 1.mkv` and `Part 1.5.mkv` -- the second begins
/// with the first's stem followed by a dot, and deleting it would cost somebody
/// a video they had downloaded. Only the shapes yt-dlp actually produces count:
/// the merged file itself, the separate streams (`.f137.mp4`) with their
/// in-progress and fragment files (`.part`, `.part-FragN`) and resume state
/// (`.ytdl`), ffmpeg's merge target (`.temp.mkv`), and the thumbnail fetched
/// for embedding. Format ids are matched as digits, which is what YouTube uses.
fn is_leftover(out_path: &Path, name: &str) -> bool {
    if out_path.file_name().and_then(|n| n.to_str()) == Some(name) {
        return true;
    }
    let Some(stem) = out_path.file_stem().and_then(|n| n.to_str()) else {
        return false;
    };
    let Some(rest) = name.strip_prefix(stem).and_then(|r| r.strip_prefix('.')) else {
        return false;
    };
    rest.ends_with(".part")
        || rest.ends_with(".ytdl")
        || rest.contains(".part-Frag")
        || rest.starts_with("temp.")
        || matches!(rest, "jpg" | "png" | "webp")
        || is_format_stream(rest)
}

/// `f137.mp4`, `f251.webm` -- one of the streams yt-dlp downloads to merge.
fn is_format_stream(rest: &str) -> bool {
    let Some((id, ext)) = rest.strip_prefix('f').and_then(|t| t.split_once('.')) else {
        return false;
    };
    !id.is_empty() && id.chars().all(|c| c.is_ascii_digit()) && !ext.is_empty()
}

/// Deletes everything a cancelled download left in the output directory, in
/// one pass. Returns what it could not delete.
fn remove_leftovers(out_path: &Path) -> Vec<PathBuf> {
    let Some(dir) = out_path.parent() else { return Vec::new() };
    let Ok(entries) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut stuck = Vec::new();
    for e in entries.flatten() {
        let name = e.file_name();
        if name.to_str().is_some_and(|n| is_leftover(out_path, n)) {
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
fn sweep_blocking(out_paths: &[PathBuf]) {
    #[allow(unused_mut, unused_variables)]
    let mut stuck: Vec<PathBuf> = out_paths.iter().flat_map(|p| remove_leftovers(p)).collect();
    #[cfg(windows)]
    for _ in 0..SWEEP_ATTEMPTS {
        if stuck.is_empty() {
            break;
        }
        std::thread::sleep(SWEEP_RETRY);
        stuck = remove_stuck(stuck);
    }
}

/// `sweep_blocking`, awaited from a blocking thread.
async fn sweep_leftovers(out_paths: Vec<PathBuf>) {
    let _ = tokio::task::spawn_blocking(move || sweep_blocking(&out_paths)).await;
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
            sweep_blocking(&out_paths);
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
struct Registry<K = proc::KillTree> {
    book: std::sync::Mutex<Book<K>>,
}

impl<K: Kill> Registry<K> {
    fn new() -> Arc<Self> {
        Arc::new(Registry {
            book: std::sync::Mutex::new(Book { jobs: HashMap::new(), cleaning: HashMap::new() }),
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
    /// `Queued`. The task first waits out any cancelled download of the same
    /// video that is still clearing up (`Book::cleaning`) -- before the
    /// semaphore, before a single file is touched.
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
        let work = body(job.clone(), alive.clone());
        let finish = Finish { reg: self.clone(), video_id: video_id.clone(), job: job.clone() };
        let handle = tokio::spawn(async move {
            // Declared first so it is dropped last: the job is not gone until
            // everything else in here has been torn down.
            let _alive = alive;
            let _finish = finish;
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
            // for -- and a queued job is parked on the semaphore behind other
            // downloads, which could be minutes.
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
        Self {
            rows: Arc::new(Ledger { db: db.clone(), app: app.clone() }),
            db,
            app,
            tools,
            sem: Arc::new(Semaphore::new(concurrency.max(1))),
            permits: Arc::new(Mutex::new(concurrency.max(1))),
            reg: Registry::new(),
            claims: Arc::new(PathClaims::default()),
        }
    }

    /// Grows the semaphore when the user raises concurrency. Lowering it takes
    /// effect as running jobs finish, since permits cannot be revoked mid-flight.
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
        let (db, app, sem) = (self.db.clone(), self.app.clone(), self.sem.clone());
        let claims = self.claims.clone();
        let tools = self.tools.clone();
        let vid = video_id.clone();
        let (reg, rows) = (self.reg.clone(), self.rows.clone());

        blocking(move || reg.enqueue(video_id, &rows, move |job, alive| async move {
            let _permit = match sem.acquire_owned().await {
                Ok(p) => p,
                Err(_) => return,
            };
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

    // The cleanup guard is declared before the child so that it drops *after*
    // it: yt-dlp is dead before a cancelled job unlinks anything. It owns the
    // claim, so the names stay reserved until its sweep is done.
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

    let mut cmd = proc::command(&inv.runner.program);
    cmd.args(ytdlp::download_args(&inv.runner, &quality, video_id, &out_path, &print_file))
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
    let stderr_text = err_collect.await.unwrap_or_default();

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
        ] {
            assert!(is_leftover(out, name), "{name} should be swept up");
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
        ] {
            assert!(!is_leftover(out, name), "{name} must survive");
        }
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

        assert!(remove_leftovers(&out).is_empty(), "nothing is held open here");

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

        assert_eq!(remove_leftovers(&out), vec![part.clone()], "held open, so it cannot go yet");
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
        let reg = Registry::<i32>::new();
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
        let reg = Registry::<i32>::new();
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
        let reg = Registry::<i32>::new();
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
        let reg = Registry::<i32>::new();
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
    async fn enqueueing_a_download_already_in_flight_does_nothing() {
        let reg = Registry::<i32>::new();
        let rows = Arc::new(FakeRows::default());
        let (tx, rx) = tokio::sync::oneshot::channel::<()>();
        reg.enqueue("v".into(), &rows, move |_j, _a| async move { let _ = rx.await; }).unwrap();
        reg.enqueue("v".into(), &rows, |_j, _a| async { panic!("a second job was started") })
            .unwrap();
        assert_eq!(*rows.sent.lock().unwrap(), vec![DownloadState::Queued]);
        tx.send(()).unwrap();
        assert!(eventually(|| !reg.is_active("v")).await);
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
