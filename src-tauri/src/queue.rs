use anyhow::{anyhow, Result};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tauri::{AppHandle, Emitter};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::sync::{Mutex, Semaphore};

use crate::db::Db;
use crate::models::{DownloadProgress, DownloadState, DownloadStateEvent};
use crate::ytdlp;

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

pub struct Queue {
    db: Arc<Db>,
    app: AppHandle,
    sem: Arc<Semaphore>,
    permits: Arc<Mutex<usize>>,
    running: Arc<Mutex<HashMap<String, tokio::task::JoinHandle<()>>>>,
    cancels: Arc<Mutex<HashMap<String, Arc<std::sync::atomic::AtomicBool>>>>,
    claims: Arc<PathClaims>,
}

impl Queue {
    pub fn new(db: Arc<Db>, app: AppHandle, concurrency: usize) -> Self {
        Self {
            db,
            app,
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
        if let Some(flag) = self.cancels.lock().await.get(video_id) {
            flag.store(true, std::sync::atomic::Ordering::SeqCst);
        }
        if let Some(h) = self.running.lock().await.remove(video_id) {
            h.abort();
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

        let cancel = Arc::new(std::sync::atomic::AtomicBool::new(false));
        self.cancels
            .lock()
            .await
            .insert(video_id.clone(), cancel.clone());

        let (db, app, sem) = (self.db.clone(), self.app.clone(), self.sem.clone());
        let running = self.running.clone();
        let cancels = self.cancels.clone();
        let claims = self.claims.clone();
        let vid = video_id.clone();

        let handle = tokio::spawn(async move {
            let _permit = match sem.acquire_owned().await {
                Ok(p) => p,
                Err(_) => return,
            };
            if cancel.load(std::sync::atomic::Ordering::SeqCst) {
                running.lock().await.remove(&vid);
                cancels.lock().await.remove(&vid);
                return;
            }
            let result = run_one(&db, &app, &vid, &dir, &template, &cancel, &claims).await;
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

async fn run_one(
    db: &Db,
    app: &AppHandle,
    video_id: &str,
    dir: &str,
    template: &str,
    cancel: &Arc<std::sync::atomic::AtomicBool>,
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

    // Phase 1: resolve the output template to a concrete path.
    let probe = ytdlp::probe(&ytdlp::watch_url(video_id), dir, template).await?;
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

    let mut child = tokio::process::Command::new("yt-dlp")
        .args(ytdlp::download_args(video_id, &out_path, &print_file))
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| anyhow!("could not run yt-dlp: {e}"))?;

    let stdout = child.stdout.take().ok_or_else(|| anyhow!("no stdout"))?;
    let stderr = child.stderr.take().ok_or_else(|| anyhow!("no stderr"))?;

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
    let _ = pump.await;
    let stderr_text = err_collect.await.unwrap_or_default();

    if cancel.load(std::sync::atomic::Ordering::SeqCst) {
        let _ = std::fs::remove_file(&print_file);
        return Ok(());
    }

    if !status.success() {
        let _ = std::fs::remove_file(&print_file);
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
    let _ = std::fs::remove_file(&print_file);

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
