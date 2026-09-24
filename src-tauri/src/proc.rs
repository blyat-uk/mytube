//! Spawning the external tools.
//!
//! Every program MyTube runs in the background goes through [`command`], so
//! that on Windows none of them flashes a console window: the release build is
//! a GUI-subsystem binary, and a console child of one gets a fresh console of
//! its own unless told otherwise.
//!
//! yt-dlp is spawned through [`spawn_tree`] on top of that, because stopping
//! it means stopping everything it started. It shells out to ffmpeg for the
//! merge and to deno for YouTube's JavaScript challenge, and the standalone
//! release is a PyInstaller "onefile" executable whose bootloader unpacks
//! itself and then runs the real yt-dlp as a *child* -- so killing the process
//! we spawned can leave the one doing the work running, on every OS. Unix puts
//! the tree in a process group of its own; Windows puts it in a Job Object.

use std::ffi::OsStr;
use std::io;
use std::process::{ExitStatus, Output, Stdio};
use std::sync::Arc;

use tokio::io::AsyncReadExt;
use tokio::process::{Child, Command};

/// `CREATE_NO_WINDOW`, from `winbase.h`.
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// A `tokio` command for a background tool: no console window on Windows.
pub fn command(program: impl AsRef<OsStr>) -> tokio::process::Command {
    #[allow(unused_mut)]
    let mut cmd = tokio::process::Command::new(program);
    #[cfg(windows)]
    cmd.creation_flags(CREATE_NO_WINDOW);
    cmd
}

/// The blocking twin of [`command`], for the few callers outside a runtime.
pub fn std_command(program: impl AsRef<OsStr>) -> std::process::Command {
    #[allow(unused_mut)]
    let mut cmd = std::process::Command::new(program);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd
}

/// The stop button for one spawned process tree. Cheap to clone and safe to
/// hand to another thread; every clone kills the same tree.
#[derive(Clone)]
pub struct KillTree {
    inner: Arc<Tree>,
}

impl KillTree {
    /// Kills every process in the tree, at once and without asking (SIGKILL,
    /// `TerminateJobObject`). Nothing yt-dlp leaves half-written is worth
    /// keeping after a cancel, and a polite signal would let it carry on
    /// merging the file that is about to be deleted.
    pub fn kill(&self) {
        self.inner.kill();
    }

    /// The leader has been reaped.
    ///
    /// On unix the group id is the leader's pid, which the kernel can hand to
    /// an unrelated process once the leader is reaped and the rest of its group
    /// is gone -- so from here on [`kill`](Self::kill) is a no-op there rather
    /// than a signal to a stranger. A Job Object is a handle of our own and
    /// cannot be reused, so on Windows `kill` still sweeps whatever is left.
    fn mark_reaped(&self) {
        #[cfg(unix)]
        self.inner.reaped.store(true, std::sync::atomic::Ordering::SeqCst);
    }
}

#[cfg(unix)]
struct Tree {
    pgid: i32,
    reaped: std::sync::atomic::AtomicBool,
}

#[cfg(unix)]
impl Tree {
    fn kill(&self) {
        if self.pgid > 0 && !self.reaped.load(std::sync::atomic::Ordering::SeqCst) {
            // Safe: `killpg` only signals, and the group is one we created, so
            // this can never reach mytube's own process.
            unsafe {
                libc::killpg(self.pgid, libc::SIGKILL);
            }
        }
    }
}

#[cfg(windows)]
struct Tree {
    job: windows_sys::Win32::Foundation::HANDLE,
}

// A Job Object handle is a kernel handle, usable from any thread; the raw
// pointer type is the only reason the compiler cannot see that for itself.
#[cfg(windows)]
unsafe impl Send for Tree {}
#[cfg(windows)]
unsafe impl Sync for Tree {}

#[cfg(windows)]
impl Tree {
    /// A fresh, unnamed job that kills everything in it when its last handle
    /// closes. The handle is not inheritable (no security attributes), which
    /// matters: a child holding it would keep the job -- and so itself -- alive.
    fn new() -> io::Result<Self> {
        use windows_sys::Win32::Foundation::CloseHandle;
        use windows_sys::Win32::System::JobObjects::{
            CreateJobObjectW, JobObjectExtendedLimitInformation, SetInformationJobObject,
            JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
        };
        unsafe {
            let job = CreateJobObjectW(std::ptr::null(), std::ptr::null());
            if job.is_null() {
                return Err(io::Error::last_os_error());
            }
            let mut info = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
            info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            let ok = SetInformationJobObject(
                job,
                JobObjectExtendedLimitInformation,
                &info as *const _ as *const core::ffi::c_void,
                std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            );
            if ok == 0 {
                let err = io::Error::last_os_error();
                CloseHandle(job);
                return Err(err);
            }
            Ok(Tree { job })
        }
    }

    fn kill(&self) {
        unsafe {
            windows_sys::Win32::System::JobObjects::TerminateJobObject(self.job, 1);
        }
    }
}

#[cfg(windows)]
impl Drop for Tree {
    /// The last clone is gone. `KILL_ON_JOB_CLOSE` makes closing the handle a
    /// kill of its own, which is the backstop for a tree nobody stopped -- and
    /// harmless for one that finished, since an empty job has nothing to kill.
    fn drop(&mut self) {
        unsafe {
            windows_sys::Win32::Foundation::CloseHandle(self.job);
        }
    }
}

/// A spawned tree whose leader has not been waited for yet.
///
/// Dropping it before [`wait`](Self::wait) has returned kills the whole tree.
/// That is what makes an aborted task or an expired timeout real: dropping a
/// future that owns a tokio `Child` otherwise leaves the child running with
/// nothing to reap it, and even `kill_on_drop` reaches the leader alone -- the
/// real yt-dlp behind PyInstaller's bootloader, and the ffmpeg it started,
/// would carry on. The kill runs in `Drop::drop`, before the `Child` field is
/// dropped, so on unix the leader is at worst an unreaped zombie when its group
/// is signalled and its pid cannot yet have gone to anybody else.
pub struct TreeChild {
    pub child: Child,
    tree: KillTree,
    reaped: bool,
}

impl TreeChild {
    /// A handle that kills this tree from elsewhere -- the cancel button.
    pub fn tree(&self) -> KillTree {
        self.tree.clone()
    }

    /// Waits for the leader and reaps it. From then on the tree is no longer
    /// killed on drop, and on unix no clone of the handle signals it either.
    pub async fn wait(&mut self) -> io::Result<ExitStatus> {
        let status = self.child.wait().await?;
        self.reaped = true;
        self.tree.mark_reaped();
        Ok(status)
    }
}

impl Drop for TreeChild {
    fn drop(&mut self) {
        if !self.reaped {
            self.tree.kill();
        }
    }
}

/// Spawns `cmd` so that its whole process tree can be killed.
///
/// Unix: the child leads a new process group (`process_group(0)`), so one
/// `killpg` reaches everything it starts. Windows: the child is assigned to a
/// fresh Job Object with `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE` straight after it
/// is created, and every process it starts from then on is born into the job.
///
/// The Windows assignment happens *after* `CreateProcess` returns, because
/// std's `Command` cannot create a process suspended or hand it a job list,
/// so there is a window in which the child runs outside any job, and a process
/// it starts inside that window would escape. For PyInstaller's bootloader --
/// the only thing MyTube runs that starts children -- the window is
/// microseconds wide, while the bootloader first unpacks its archive to a
/// temporary directory, which takes tens to hundreds of milliseconds, before
/// it starts the real yt-dlp. The race cannot be won by the child in practice,
/// and closing it would take `STARTUPINFOEX` plumbing std does not expose.
///
/// `kill_on_drop` is set as well, as the backstop for the leader.
pub fn spawn_tree(cmd: &mut Command) -> io::Result<TreeChild> {
    cmd.kill_on_drop(true);

    #[cfg(unix)]
    {
        cmd.process_group(0);
        let child = cmd.spawn()?;
        // The group id is the leader's pid. A child with no pid has already
        // been reaped, which cannot happen before we have waited for it.
        let pgid = child.id().map(|p| p as i32).unwrap_or(0);
        let tree = KillTree {
            inner: Arc::new(Tree { pgid, reaped: Default::default() }),
        };
        Ok(TreeChild { child, tree, reaped: false })
    }

    #[cfg(windows)]
    {
        use windows_sys::Win32::System::JobObjects::AssignProcessToJobObject;

        let tree = Tree::new()?;
        let mut child = cmd.spawn()?;
        // `raw_handle` is `None` only once the child has been waited for.
        if let Some(handle) = child.raw_handle() {
            let ok = unsafe { AssignProcessToJobObject(tree.job, handle as _) };
            if ok == 0 {
                let err = io::Error::last_os_error();
                // A child that has already exited cannot be assigned, and has
                // nothing left to kill either: carry on, and let `wait` collect
                // its status. One still running outside the job would be a
                // tree we could not stop, so that is refused outright.
                if !matches!(child.try_wait(), Ok(Some(_))) {
                    let _ = child.start_kill();
                    return Err(err);
                }
            }
        }
        let tree = KillTree { inner: Arc::new(tree) };
        Ok(TreeChild { child, tree, reaped: false })
    }
}

/// Runs `cmd` to completion and captures its output, like `Command::output`,
/// except that dropping the future early -- a timeout, an aborted task --
/// kills the whole tree rather than orphaning it. stdin is closed.
pub async fn output(cmd: &mut Command) -> io::Result<Output> {
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut tc = spawn_tree(cmd)?;
    let mut stdout = tc.child.stdout.take();
    let mut stderr = tc.child.stderr.take();

    // Both pipes are drained while waiting, not after: a child that fills one
    // pipe's buffer blocks until somebody reads it, and would never exit.
    async fn drain<R: tokio::io::AsyncRead + Unpin>(r: Option<&mut R>) -> io::Result<Vec<u8>> {
        let mut buf = Vec::new();
        if let Some(r) = r {
            r.read_to_end(&mut buf).await?;
        }
        Ok(buf)
    }
    let (out, err) = tokio::try_join!(drain(stdout.as_mut()), drain(stderr.as_mut()))?;
    let status = tc.wait().await?;
    Ok(Output { status, stdout: out, stderr: err })
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::time::Duration;

    /// A unique `sleep` duration, so `pgrep` finds this test's own children and
    /// nobody else's. The process id goes after the decimal point so it stays a
    /// number `sleep` accepts.
    fn secs(base: u32) -> String {
        format!("{base}.{}", std::process::id())
    }

    fn alive(pattern: &str) -> bool {
        let out = std::process::Command::new("pgrep")
            .arg("-f")
            .arg(pattern)
            .output()
            .expect("pgrep is available");
        !out.stdout.is_empty()
    }

    async fn wait_until(want: bool, pattern: &str) -> bool {
        for _ in 0..80 {
            if alive(pattern) == want {
                return true;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        false
    }

    fn sh_tree(secs: &str) -> Command {
        let mut cmd = command("sh");
        // Two sleeps: one a background grandchild, one keeping sh itself busy.
        cmd.arg("-c").arg(format!("sleep {secs} & sleep {secs}"));
        cmd
    }

    #[tokio::test]
    async fn killing_the_tree_reaches_the_grandchild_not_just_the_leader() {
        let secs = secs(302);
        let mut tc = spawn_tree(&mut sh_tree(&secs)).expect("sh is available");
        assert!(wait_until(true, &secs).await, "the sleeps should be running first");

        tc.tree().kill();
        let _ = tc.wait().await;

        let gone = wait_until(false, &secs).await;
        let _ = std::process::Command::new("pkill").arg("-f").arg(&secs).status();
        assert!(gone, "a grandchild outlived the kill");
    }

    #[tokio::test]
    async fn dropping_an_unreaped_tree_kills_all_of_it() {
        // An aborted task or an expired timeout drops the child without ever
        // waiting for it. That has to stop the grandchild too -- `kill_on_drop`
        // alone would reach the leader and nothing under it.
        let secs = secs(303);
        let tc = spawn_tree(&mut sh_tree(&secs)).expect("sh is available");
        assert!(wait_until(true, &secs).await, "the sleeps should be running first");

        drop(tc);

        let gone = wait_until(false, &secs).await;
        let _ = std::process::Command::new("pkill").arg("-f").arg(&secs).status();
        assert!(gone, "a grandchild outlived the dropped tree");
    }

    #[tokio::test]
    async fn a_reaped_tree_is_never_signalled_again() {
        // Once the leader is reaped its pid -- the group id -- is free for the
        // kernel to reuse, so a late cancel must not signal whoever has it now.
        let mut tc = spawn_tree(&mut command("true")).expect("true is available");
        let tree = tc.tree();
        assert!(tc.wait().await.unwrap().success());
        assert!(tree.inner.reaped.load(std::sync::atomic::Ordering::SeqCst));
        tree.kill(); // a no-op, and must not panic
    }

    #[tokio::test]
    async fn output_captures_both_streams_and_the_status() {
        let mut cmd = command("sh");
        cmd.arg("-c").arg("echo out; echo err >&2; exit 3");
        let out = output(&mut cmd).await.expect("sh is available");
        assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "out");
        assert_eq!(String::from_utf8_lossy(&out.stderr).trim(), "err");
        assert_eq!(out.status.code(), Some(3));
    }

    #[tokio::test]
    async fn output_dropped_by_a_timeout_kills_the_whole_tree() {
        let secs = secs(304);
        let mut cmd = sh_tree(&secs);
        let res = tokio::time::timeout(Duration::from_millis(300), output(&mut cmd)).await;
        assert!(res.is_err(), "a 300s sleep must not finish inside 300ms");

        let gone = wait_until(false, &secs).await;
        let _ = std::process::Command::new("pkill").arg("-f").arg(&secs).status();
        assert!(gone, "a grandchild outlived the timeout");
    }
}

#[cfg(all(test, windows))]
mod windows_tests {
    use super::*;
    use std::time::Duration;
    use windows_sys::Win32::System::JobObjects::{
        JobObjectBasicProcessIdList, QueryInformationJobObject,
    };

    /// The pids currently in the job, read from the kernel.
    fn pids_in(tree: &KillTree) -> Vec<usize> {
        // JOBOBJECT_BASIC_PROCESS_ID_LIST is two u32 counts followed by a
        // variable-length array of ULONG_PTR; room for 64 is plenty here.
        let mut buf = vec![0usize; 2 + 64];
        let ok = unsafe {
            QueryInformationJobObject(
                tree.inner.job,
                JobObjectBasicProcessIdList,
                buf.as_mut_ptr() as *mut core::ffi::c_void,
                (buf.len() * std::mem::size_of::<usize>()) as u32,
                std::ptr::null_mut(),
            )
        };
        assert_ne!(ok, 0, "QueryInformationJobObject failed: {}", io::Error::last_os_error());
        let listed = unsafe { *(buf.as_ptr() as *const u32).add(1) } as usize;
        // The array starts 8 bytes in, which is buf[1] on 64-bit.
        let first = 8 / std::mem::size_of::<usize>();
        buf[first..first + listed].to_vec()
    }

    fn running(pid: usize) -> bool {
        let out = std::process::Command::new("tasklist")
            .args(["/FI", &format!("PID eq {pid}"), "/NH", "/FO", "CSV"])
            .output()
            .expect("tasklist is available");
        String::from_utf8_lossy(&out.stdout).contains(&format!("\"{pid}\""))
    }

    #[tokio::test]
    async fn killing_the_job_reaches_the_grandchild_not_just_the_leader() {
        // cmd starts ping as a child of its own: the same shape as PyInstaller's
        // bootloader starting the real yt-dlp.
        let mut cmd = command("cmd");
        cmd.args(["/C", "ping -n 300 127.0.0.1 > NUL"]);
        let mut tc = spawn_tree(&mut cmd).expect("cmd is available");
        let tree = tc.tree();

        let mut pids = Vec::new();
        for _ in 0..100 {
            pids = pids_in(&tree);
            if pids.len() >= 2 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        assert!(pids.len() >= 2, "cmd and ping should both be in the job: {pids:?}");
        assert!(pids.iter().all(|&p| running(p)), "tasklist should see {pids:?}");

        tree.kill();
        // Bounded, so a kill that did nothing fails the test instead of
        // hanging it for the 300 seconds ping would take.
        let reaped = tokio::time::timeout(Duration::from_secs(10), tc.wait()).await;
        assert!(reaped.is_ok(), "cmd outlived the job kill");

        for _ in 0..100 {
            if pids.iter().all(|&p| !running(p)) {
                return;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        panic!("something in {pids:?} outlived the job kill");
    }
}
