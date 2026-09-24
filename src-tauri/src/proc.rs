//! Spawning the external tools.
//!
//! Every program MyTube runs in the background goes through [`command`], so
//! that on Windows none of them flashes a console window: the release build is
//! a GUI-subsystem binary, and a console child of one gets a fresh console of
//! its own unless told otherwise.

use std::ffi::OsStr;

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
