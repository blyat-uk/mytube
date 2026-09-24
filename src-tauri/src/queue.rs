use anyhow::{anyhow, Result};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tauri::{AppHandle, Emitter};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::sync::{Mutex, Semaphore};

use crate::db::Db;
use crate::models::{DownloadProgress, DownloadState, DownloadStateEvent};
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
        let mut guard = self.inner.lock().unwrap();
        let chosen = ytdlp::unique_path(intended, |c| guard.contains(c) || on_disk(c));
        guard.insert(chosen.clone());
        chosen
    }
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

/// Deletes everything a cancelled download left in the output directory.
fn remove_leftovers(out_path: &Path) {
    let Some(dir) = out_path.parent() else { return };
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for e in entries.flatten() {
        let name = e.file_name();
        if name.to_str().is_some_and(|n| is_leftover(out_path, n)) {
            let _ = std::fs::remove_file(e.path());
        }
    }
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
    out_path: PathBuf,
    print_file: PathBuf,
}
impl Drop for CleanupGuard {
    fn drop(&mut self) {
        // Removed on every exit path, not just cancellation: an aborted job
        // used to leave one of these in /tmp for the life of the machine.
        let _ = std::fs::remove_file(&self.print_file);
        if self.job.is_cancelled() {
            remove_leftovers(&self.out_path);
        }
    }
}

/// Releases a claimed path however the job ends, including on panic or abort.
struct ClaimGuard {
    claims: Arc<PathClaims>,
    path: PathBuf,
}
impl Drop for ClaimGuard {
    fn drop(&mut self) {
        self.claims.release(&self.path);
    }
}

/// How long `cancel` gives a running download to kill yt-dlp, reap it and
/// delete its part files. That is milliseconds of real work; the allowance is
/// wide only so an unlink stalled on a busy disk is not cut short.
const CLEANUP_GRACE: std::time::Duration = std::time::Duration::from_secs(15);

pub struct Queue {
    db: Arc<Db>,
    app: AppHandle,
    sem: Arc<Semaphore>,
    permits: Arc<Mutex<usize>>,
    running: Arc<Mutex<HashMap<String, tokio::task::JoinHandle<()>>>>,
    cancels: Arc<Mutex<HashMap<String, Arc<Job>>>>,
    claims: Arc<PathClaims>,
    tools: Arc<Tools>,
}

impl Queue {
    pub fn new(db: Arc<Db>, app: AppHandle, concurrency: usize, tools: Arc<Tools>) -> Self {
        Self {
            db,
            app,
            tools,
            sem: Arc::new(Semaphore::new(concurrency.max(1))),
            permits: Arc::new(Mutex::new(concurrency.max(1))),
            running: Arc::new(Mutex::new(HashMap::new())),
            cancels: Arc::new(Mutex::new(HashMap::new())),
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

    pub async fn cancel(&self, video_id: &str) -> Result<()> {
        // Stop yt-dlp first, and mark the job cancelled in the same breath, so
        // whatever the job does next it knows to clear up rather than record a
        // file the library is about to forget about.
        let job = self.cancels.lock().await.get(video_id).cloned();
        let stop = job.map(|j| j.cancel());
        if let Some(tree) = stop.as_ref().and_then(|s| s.tree.as_ref()) {
            tree.kill();
        }

        if let Some(h) = self.running.lock().await.remove(video_id) {
            if stop.is_some_and(|s| s.spawned) {
                // Something may be on disk, so wait for the job to sweep it up
                // -- a cancel that has returned should mean the download really
                // is gone. Aborting instead (which is what this used to do)
                // skipped that cleanup and, worse, left yt-dlp running: tokio
                // does not kill a child when its task is dropped, so it ran on
                // to a full download that nothing then recorded or deleted.
                let abort = h.abort_handle();
                if tokio::time::timeout(CLEANUP_GRACE, h).await.is_err() {
                    abort.abort();
                }
            } else {
                // Nothing has been spawned, so there is nothing on disk and
                // nothing to wait for -- and a queued job is parked on the
                // semaphore behind other downloads, which could be minutes.
                h.abort();
            }
        }
        self.cancels.lock().await.remove(video_id);
        self.db
            .set_download_state(video_id, DownloadState::None, None, None)?;
        let _ = self.app.emit(
            "download://state",
            DownloadStateEvent {
                video_id: video_id.to_string(),
                state: DownloadState::None,
                file_path: None,
                error: None,
            },
        );
        Ok(())
    }

    pub async fn enqueue(&self, video_id: String, dir: String, template: String) -> Result<()> {
        if self.running.lock().await.contains_key(&video_id) {
            return Ok(()); // already in flight
        }
        self.db
            .set_download_state(&video_id, DownloadState::Queued, None, None)?;
        let _ = self.app.emit(
            "download://state",
            DownloadStateEvent {
                video_id: video_id.clone(),
                state: DownloadState::Queued,
                file_path: None,
                error: None,
            },
        );

        let cancel = Arc::new(Job::default());
        self.cancels
            .lock()
            .await
            .insert(video_id.clone(), cancel.clone());

        let (db, app, sem) = (self.db.clone(), self.app.clone(), self.sem.clone());
        let running = self.running.clone();
        let cancels = self.cancels.clone();
        let claims = self.claims.clone();
        let tools = self.tools.clone();
        let vid = video_id.clone();

        let handle = tokio::spawn(async move {
            let _permit = match sem.acquire_owned().await {
                Ok(p) => p,
                Err(_) => return,
            };
            if cancel.is_cancelled() {
                running.lock().await.remove(&vid);
                cancels.lock().await.remove(&vid);
                return;
            }
            let result = run_one(&db, &app, &tools, &vid, &dir, &template, &cancel, &claims).await;
            if let Err(e) = result {
                let msg = tail(&e.to_string(), 2000);
                let _ = db.set_download_state(&vid, DownloadState::Failed, None, Some(&msg));
                let _ = app.emit(
                    "download://state",
                    DownloadStateEvent {
                        video_id: vid.clone(),
                        state: DownloadState::Failed,
                        file_path: None,
                        error: Some(msg),
                    },
                );
            }
            running.lock().await.remove(&vid);
            cancels.lock().await.remove(&vid);
        });

        self.running.lock().await.insert(video_id, handle);
        Ok(())
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
    claims: &Arc<PathClaims>,
) -> Result<()> {
    std::fs::create_dir_all(dir)?;
    let print_file = std::env::temp_dir().join(format!("mytube-{video_id}.path"));
    let _ = std::fs::remove_file(&print_file);

    db.set_download_state(video_id, DownloadState::Downloading, None, None)?;
    let _ = app.emit(
        "download://state",
        DownloadStateEvent {
            video_id: video_id.to_string(),
            state: DownloadState::Downloading,
            file_path: None,
            error: None,
        },
    );

    // One yt-dlp for both phases, resolved now rather than at enqueue so a job
    // that sat in the queue gets whatever is installed when it starts. Its
    // lease is held until the download is reaped: the updater never swaps
    // yt-dlp while a job is using it. Not available yet is an ordinary failure.
    let inv = tools.ytdlp(&config::load().unwrap_or_default()).await?;

    // Phase 1: resolve the output template to a concrete path.
    let probe = ytdlp::probe(&inv, &ytdlp::watch_url(video_id), dir, template).await?;
    let intended = PathBuf::from(&probe.intended_path);
    if intended.as_os_str().is_empty() {
        return Err(anyhow!("yt-dlp could not resolve an output filename"));
    }

    // Phase 2: claim a free path, appending " (N)" if this name is taken.
    let out_path = claims.claim_unique(&intended, |c| c.exists());
    let _guard = ClaimGuard {
        claims: claims.clone(),
        path: out_path.clone(),
    };
    if let Some(parent) = out_path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    // The cleanup guard is declared before the child so that it drops *after*
    // it: yt-dlp is dead before a cancelled job unlinks anything.
    let _cleanup = CleanupGuard {
        job: cancel.clone(),
        out_path: out_path.clone(),
        print_file: print_file.clone(),
    };

    let mut cmd = proc::command(&inv.runner.program);
    cmd.args(ytdlp::download_args(&inv.runner, video_id, &out_path, &print_file))
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
    let pump = tokio::spawn(async move {
        let mut lines = BufReader::new(stdout).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            if let Some((percent, speed, eta)) = ytdlp::parse_progress_line(&line) {
                let _ = app2.emit(
                    "download://progress",
                    DownloadProgress {
                        video_id: vid2.clone(),
                        percent,
                        speed,
                        eta,
                    },
                );
            }
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

    // `_cleanup` deletes the part files on the way out; the row is left alone
    // because `Queue::cancel` owns it.
    if cancel.is_cancelled() {
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

    let path = std::fs::read_to_string(&print_file)
        .map_err(|_| anyhow!("yt-dlp finished but reported no output file"))?
        .lines()
        .last()
        .unwrap_or_default()
        .trim()
        .to_string();

    if path.is_empty() || !Path::new(&path).exists() {
        return Err(anyhow!("yt-dlp finished but the output file is missing"));
    }

    db.set_download_state(video_id, DownloadState::Done, Some(&path), None)?;
    let _ = app.emit(
        "download://state",
        DownloadStateEvent {
            video_id: video_id.to_string(),
            state: DownloadState::Done,
            file_path: Some(path),
            error: None,
        },
    );
    Ok(())
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

        remove_leftovers(&out);

        let mut left: Vec<String> = std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        left.sort();
        assert_eq!(left, vec!["Part 1.5.mkv".to_string(), "Part 2.mkv".to_string()]);
        let _ = std::fs::remove_dir_all(&dir);
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
