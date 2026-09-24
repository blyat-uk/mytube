//! The three external programs MyTube runs -- yt-dlp, ffmpeg (with ffprobe) and
//! deno -- found, downloaded, verified and kept up to date.
//!
//! FOUNDATION STUB: the signatures below are the contract other modules build
//! against. The tools task replaces every body.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::Result;
use tauri::AppHandle;
use tokio::sync::OwnedRwLockReadGuard;

use crate::config::Settings;
use crate::models::ToolStatus;
use crate::ytdlp::Runner;

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

pub struct Tools {
    _private: (),
}

impl Tools {
    pub fn new(bin_dir: PathBuf, http: reqwest::Client) -> Arc<Self> {
        let _ = (bin_dir, http);
        todo!("tools task")
    }

    /// Lets status and progress reach the UI. Before this is called, nothing is
    /// emitted (the CLI self-test never calls it).
    pub fn set_app(&self, app: AppHandle) {
        let _ = app;
        todo!("tools task")
    }

    /// Resolves yt-dlp, ffmpeg and deno for `s` and takes a lease. An `Err`
    /// reads "yt-dlp is not available yet: <why>".
    pub async fn ytdlp(&self, s: &Settings) -> Result<Invocation> {
        let _ = s;
        todo!("tools task")
    }

    /// Downloads whatever resolves to nothing. Never fails: errors land in the
    /// status. Called at startup and on every poll tick.
    pub async fn ensure_all(&self, s: &Settings) {
        let _ = s;
        todo!("tools task")
    }

    /// The daily yt-dlp update check (managed copy only, `ytdlp_auto_update`).
    pub async fn maybe_update(&self, s: &Settings) {
        let _ = s;
        todo!("tools task")
    }

    /// "Check for updates" / "Retry": forces the check and retries failed
    /// installs. `Err("busy …")` while any lease is held.
    pub async fn update_now(&self, s: &Settings) -> Result<Vec<ToolStatus>> {
        let _ = s;
        todo!("tools task")
    }

    pub async fn status(&self, s: &Settings) -> Vec<ToolStatus> {
        let _ = s;
        todo!("tools task")
    }
}

/// `mytube --self-test <dir>`: provisions all three tools fresh from their
/// real sources into `dir`, ignoring any system copy, checks each runs, and
/// that yt-dlp reports deno as a JS runtime. Returns the report text.
pub async fn self_test(dir: &Path) -> Result<String> {
    let _ = dir;
    todo!("tools task")
}
