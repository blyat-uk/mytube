//! Getting a verified tool into `bin/`: download into `bin/.staging/`, hash
//! while streaming, compare against the publisher's SHA-256, extract only the
//! executables we run, and only then move them into place.
//!
//! The invariant every step keeps: **nothing reaches its final name in `bin/`
//! until it has been verified, and the final step is a rename.** A download
//! cut off halfway, a checksum mismatch, a full disk or the app quitting leave
//! at most a `.part` in `.staging/` -- which the resolver never looks at and
//! `sweep` deletes at the next start -- and the previous binary exactly where
//! it was.

use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::{anyhow, bail, Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::io::AsyncWriteExt;

use crate::models::{ToolKind, ToolPhase, ToolProgress};

/// A tool download is up to 200 MB (the Windows ffmpeg zip), so the shared
/// client's 30 s *total* timeout would kill any of them on an ordinary line.
/// Each request here gets this ceiling instead, and the real guard is
/// [`IDLE_TIMEOUT`]: a transfer that is still moving is never cut off.
const DOWNLOAD_CEILING: Duration = Duration::from_secs(6 * 60 * 60);
/// No bytes for this long and the download is abandoned as stalled.
const IDLE_TIMEOUT: Duration = Duration::from_secs(60);
/// `tools://progress` at most ten times a second.
const PROGRESS_EVERY: Duration = Duration::from_millis(100);

pub const STAGING: &str = ".staging";
pub const MANIFEST: &str = "tools.json";

/// How the expected hash is found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Checksum {
    /// A `sha256sum`-style list naming many files; take `name`'s line.
    InList { url: String, name: String },
    /// A file describing just this download.
    Single { url: String },
    /// `<the URL the download finally landed on><suffix>`: martin-riedl's
    /// redirect resolves to a versioned path, and the hash sits beside that.
    BesideFinal { suffix: &'static str },
}

/// What to keep from a download: `(name inside, name in bin/)` pairs. An
/// archive member matches on its file name alone, wherever it sits
/// (`ffmpeg-master-latest-linux64-gpl/bin/ffmpeg`); nothing else is written,
/// so an archive cannot place a file anywhere we did not name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Unpack {
    /// The download is the executable.
    Bare { dest: String },
    Zip { take: Vec<(String, String)> },
    TarXz { take: Vec<(String, String)> },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fetch {
    pub url: String,
    pub checksum: Checksum,
    pub unpack: Unpack,
}

/// Everything needed to install one tool. `version` is what `tools.json`
/// records when it is known up front (a yt-dlp or deno tag).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    pub kind: ToolKind,
    pub version: Option<String>,
    pub fetches: Vec<Fetch>,
}

/// Verified executables waiting in `.staging/`, and where each goes.
#[derive(Debug)]
pub struct Staged {
    files: Vec<(PathBuf, String)>,
}

impl Staged {
    /// Moves every staged file to its final name, never leaving a moment in
    /// which the tool is missing: the old binary is only ever deleted once the
    /// new one is in its place.
    ///
    /// - unix: one `rename(new, final)`, which replaces the old file
    ///   atomically. A running copy keeps its inode, so nothing needs moving
    ///   aside first.
    /// - Windows: a running exe can be renamed but not overwritten, so the old
    ///   one goes to `<name>.old` first, then the new one to the final name --
    ///   and if *that* fails the old one is renamed straight back. `.old` is
    ///   deleted only after success, best effort: a running exe cannot be
    ///   deleted, and `sweep` gets it at the next start.
    ///
    /// On failure the files not yet placed are removed from `.staging/`.
    pub fn place(self, bin: &Path) -> Result<Vec<PathBuf>> {
        self.place_with(bin, &|from, to| std::fs::rename(from, to))
    }

    /// [`place`](Self::place) with the rename supplied, so a test can make
    /// one of them fail.
    fn place_with(self, bin: &Path, rename: &dyn Fn(&Path, &Path) -> io::Result<()>) -> Result<Vec<PathBuf>> {
        let mut placed = Vec::new();
        for (i, (from, dest)) in self.files.iter().enumerate() {
            let to = bin.join(dest);
            if let Err(e) = place_one(bin, from, dest, &to, rename) {
                for (p, _) in &self.files[i..] {
                    let _ = std::fs::remove_file(p);
                }
                return Err(e);
            }
            placed.push(to);
        }
        Ok(placed)
    }

    pub fn discard(self) {
        for (p, _) in &self.files {
            let _ = std::fs::remove_file(p);
        }
    }
}

#[cfg(not(windows))]
fn place_one(_bin: &Path, from: &Path, dest: &str, to: &Path, rename: &dyn Fn(&Path, &Path) -> io::Result<()>) -> Result<()> {
    rename(from, to).with_context(|| format!("installing {dest}"))
}

#[cfg(windows)]
fn place_one(bin: &Path, from: &Path, dest: &str, to: &Path, rename: &dyn Fn(&Path, &Path) -> io::Result<()>) -> Result<()> {
    if !to.exists() {
        return rename(from, to).with_context(|| format!("installing {dest}"));
    }
    let old = free_old_name(bin, dest);
    rename(to, &old).with_context(|| format!("moving the old {dest} aside"))?;
    if let Err(e) = rename(from, to) {
        // Put the working binary back before reporting: a failed update must
        // leave the tool exactly as it was, not missing.
        let _ = rename(&old, to);
        return Err(e).with_context(|| format!("installing {dest}"));
    }
    let _ = std::fs::remove_file(&old);
    Ok(())
}

/// `<dest>.old`, or `<dest>.<n>.old` when an earlier `.old` is still held
/// open by a process that has not exited (Windows).
#[cfg(windows)]
fn free_old_name(bin: &Path, dest: &str) -> PathBuf {
    let first = bin.join(format!("{dest}.old"));
    if !first.exists() || std::fs::remove_file(&first).is_ok() {
        return first;
    }
    let stamp = chrono::Utc::now().timestamp_millis();
    bin.join(format!("{dest}.{stamp}.old"))
}

/// Deletes what an interrupted install or a replaced binary left behind:
/// the whole `.staging/` and every `*.old`. Best effort -- a still-running old
/// exe on Windows stays until the next start.
pub fn sweep(bin: &Path) {
    let _ = std::fs::remove_dir_all(bin.join(STAGING));
    if let Ok(rd) = std::fs::read_dir(bin) {
        for e in rd.flatten() {
            if e.file_name().to_string_lossy().ends_with(".old") {
                let _ = std::fs::remove_file(e.path());
            }
        }
    }
}

/// Downloads, verifies and extracts every fetch of `plan` into `.staging/`.
/// On any failure everything it staged is removed and nothing in `bin/` has
/// been touched.
pub async fn stage(
    http: &reqwest::Client,
    bin: &Path,
    plan: &Plan,
    progress: &(dyn Fn(ToolProgress) + Send + Sync),
) -> Result<Staged> {
    let staging = bin.join(STAGING);
    tokio::fs::create_dir_all(&staging)
        .await
        .with_context(|| format!("creating {}", staging.display()))?;
    let mut staged = Staged { files: Vec::new() };
    for (i, f) in plan.fetches.iter().enumerate() {
        if let Err(e) = stage_one(http, &staging, plan.kind, i, f, progress, &mut staged).await {
            staged.discard();
            return Err(e);
        }
    }
    Ok(staged)
}

async fn stage_one(
    http: &reqwest::Client,
    staging: &Path,
    kind: ToolKind,
    index: usize,
    f: &Fetch,
    progress: &(dyn Fn(ToolProgress) + Send + Sync),
    staged: &mut Staged,
) -> Result<()> {
    let asset = asset_name(&f.url);
    // Known before the download where the publisher allows it, so a missing
    // checksum costs a few bytes rather than 150 MB.
    let expected = match &f.checksum {
        Checksum::InList { url, name } => {
            let text = fetch_text(http, url).await?;
            Some(
                super::sources::hash_in_list(&text, name)
                    .ok_or_else(|| anyhow!("{url} lists no checksum for {name}"))?,
            )
        }
        Checksum::Single { url } => {
            let text = fetch_text(http, url).await?;
            Some(super::sources::single_hash(&text).ok_or_else(|| anyhow!("{url} holds no SHA-256"))?)
        }
        Checksum::BesideFinal { .. } => None,
    };

    // The index keeps two fetches of one plan (martin-riedl's ffmpeg.zip and
    // ffprobe.zip) apart even if they ever shared a name.
    let part = staging.join(format!("{}-{index}-{asset}.part", kind.label()));
    let _ = tokio::fs::remove_file(&part).await;
    let result = download(http, &f.url, &part, kind, progress).await;
    let (actual, final_url) = match result {
        Ok(v) => v,
        Err(e) => {
            let _ = tokio::fs::remove_file(&part).await;
            return Err(e);
        }
    };

    let expected = match (expected, &f.checksum) {
        (Some(h), _) => h,
        (None, Checksum::BesideFinal { suffix }) => {
            let url = format!("{final_url}{suffix}");
            match fetch_text(http, &url).await.and_then(|t| {
                super::sources::single_hash(&t).ok_or_else(|| anyhow!("{url} holds no SHA-256"))
            }) {
                Ok(h) => h,
                Err(e) => {
                    let _ = tokio::fs::remove_file(&part).await;
                    return Err(e);
                }
            }
        }
        (None, _) => unreachable!("only BesideFinal defers its checksum"),
    };
    if actual != expected {
        let _ = tokio::fs::remove_file(&part).await;
        bail!("checksum mismatch for {asset}: expected {expected}, got {actual}");
    }

    let outcome = unpack(staging, &part, &f.unpack, kind, progress).await;
    let _ = tokio::fs::remove_file(&part).await;
    staged.files.extend(outcome?);
    Ok(())
}

/// The last path segment of a URL, for messages and staging names.
fn asset_name(url: &str) -> String {
    let path = url.split(['?', '#']).next().unwrap_or(url);
    let name = path.rsplit('/').next().unwrap_or("download");
    let clean: String = name
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || "._-".contains(c) { c } else { '_' })
        .collect();
    if clean.is_empty() { "download".into() } else { clean }
}

/// A small text file (a checksum list, a `.sha256`), with the client's own
/// 30 s timeout.
pub async fn fetch_text(http: &reqwest::Client, url: &str) -> Result<String> {
    let resp = http.get(url).send().await.with_context(|| format!("fetching {url}"))?;
    let status = resp.status();
    if !status.is_success() {
        bail!("{url} answered {status}");
    }
    resp.text().await.with_context(|| format!("reading {url}"))
}

/// Streams `url` into `dest`, hashing as it goes. Returns the lowercase
/// SHA-256 and the URL the redirects finally landed on.
async fn download(
    http: &reqwest::Client,
    url: &str,
    dest: &Path,
    kind: ToolKind,
    progress: &(dyn Fn(ToolProgress) + Send + Sync),
) -> Result<(String, String)> {
    let mut resp = http
        .get(url)
        .timeout(DOWNLOAD_CEILING)
        .send()
        .await
        .with_context(|| format!("downloading {url}"))?;
    let status = resp.status();
    if !status.is_success() {
        bail!("{url} answered {status}");
    }
    let final_url = resp.url().to_string();
    let total = resp.content_length();
    let mut file = tokio::fs::File::create(dest)
        .await
        .with_context(|| format!("creating {}", dest.display()))?;
    let mut hasher = Sha256::new();
    let mut received: u64 = 0;
    let mut last = Instant::now() - PROGRESS_EVERY;
    let report = |received| ToolProgress { tool: kind, phase: ToolPhase::Download, received, total };
    progress(report(0));
    loop {
        let chunk = match tokio::time::timeout(IDLE_TIMEOUT, resp.chunk()).await {
            Err(_) => bail!("download of {url} stalled: nothing for {}s", IDLE_TIMEOUT.as_secs()),
            Ok(r) => r.with_context(|| format!("downloading {url}"))?,
        };
        let Some(chunk) = chunk else { break };
        hasher.update(&chunk);
        file.write_all(&chunk).await.with_context(|| format!("writing {}", dest.display()))?;
        received += chunk.len() as u64;
        if last.elapsed() >= PROGRESS_EVERY {
            last = Instant::now();
            progress(report(received));
        }
    }
    if let Some(t) = total {
        // reqwest already errors on a body shorter than Content-Length; this
        // is the belt to that brace, since a short file must never verify.
        if received != t {
            bail!("download of {url} ended at {received} of {t} bytes");
        }
    }
    file.flush().await?;
    file.sync_all().await?;
    drop(file);
    progress(report(received));
    Ok((hex(&hasher.finalize()), final_url))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Extracts (or renames) the wanted executables to `.staging/<dest>.new`,
/// marked executable.
async fn unpack(
    staging: &Path,
    part: &Path,
    how: &Unpack,
    kind: ToolKind,
    progress: &(dyn Fn(ToolProgress) + Send + Sync),
) -> Result<Vec<(PathBuf, String)>> {
    let out: Vec<(PathBuf, String)> = match how {
        Unpack::Bare { dest } => {
            let to = staging.join(format!("{dest}.new"));
            tokio::fs::rename(part, &to).await?;
            vec![(to, dest.clone())]
        }
        Unpack::Zip { take } | Unpack::TarXz { take } => {
            progress(ToolProgress { tool: kind, phase: ToolPhase::Extract, received: 0, total: None });
            let wanted: Vec<(String, String, PathBuf)> = take
                .iter()
                .map(|(member, dest)| {
                    (member.clone(), dest.clone(), staging.join(format!("{dest}.new")))
                })
                .collect();
            let part = part.to_path_buf();
            let is_zip = matches!(how, Unpack::Zip { .. });
            let w = wanted.clone();
            let extracted = tokio::task::spawn_blocking(move || {
                if is_zip {
                    extract_zip(&part, &w)
                } else {
                    extract_tar_xz(&part, &w)
                }
            })
            .await
            .map_err(|e| anyhow!("extraction task failed: {e}"))
            .and_then(|r| r);
            if let Err(e) = extracted {
                // Members written before the failure are not staged yet, so
                // nothing else would clean them up.
                for (_, _, p) in &wanted {
                    let _ = std::fs::remove_file(p);
                }
                return Err(e);
            }
            wanted.into_iter().map(|(_, dest, to)| (to, dest)).collect()
        }
    };
    if let Err(e) = out.iter().try_for_each(|(p, _)| make_executable(p)) {
        for (p, _) in &out {
            let _ = std::fs::remove_file(p);
        }
        return Err(e);
    }
    Ok(out)
}

fn base_name(path: &str) -> &str {
    path.rsplit(['/', '\\']).next().unwrap_or(path)
}

/// Pulls each wanted member (by file name) out of a zip. Every wanted
/// member must be present.
fn extract_zip(archive: &Path, wanted: &[(String, String, PathBuf)]) -> Result<()> {
    let file = std::fs::File::open(archive)?;
    let mut zip = zip::ZipArchive::new(io::BufReader::new(file)).context("reading the zip")?;
    let mut done = vec![false; wanted.len()];
    for i in 0..zip.len() {
        let mut entry = zip.by_index(i)?;
        if !entry.is_file() {
            continue;
        }
        let name = entry.name().to_string();
        if let Some(k) = wanted.iter().position(|(m, _, _)| m == base_name(&name)) {
            if done[k] {
                continue;
            }
            let mut out = std::fs::File::create(&wanted[k].2)?;
            io::copy(&mut entry, &mut out).with_context(|| format!("extracting {name}"))?;
            out.sync_all()?;
            done[k] = true;
        }
    }
    missing_members(wanted, &done)
}

/// The same for a `.tar.xz`, decoded in pure Rust.
fn extract_tar_xz(archive: &Path, wanted: &[(String, String, PathBuf)]) -> Result<()> {
    let file = io::BufReader::new(std::fs::File::open(archive)?);
    let xz = lzma_rust2::XzReader::new(file, true);
    let mut tar = tar::Archive::new(xz);
    let mut done = vec![false; wanted.len()];
    for entry in tar.entries().context("reading the tarball")? {
        let mut entry = entry.context("reading the tarball")?;
        if !entry.header().entry_type().is_file() {
            continue;
        }
        let name = entry.path()?.to_string_lossy().into_owned();
        if let Some(k) = wanted.iter().position(|(m, _, _)| m == base_name(&name)) {
            if done[k] {
                continue;
            }
            let mut out = std::fs::File::create(&wanted[k].2)?;
            io::copy(&mut entry, &mut out).with_context(|| format!("extracting {name}"))?;
            out.sync_all()?;
            done[k] = true;
        }
        if done.iter().all(|d| *d) {
            break;
        }
    }
    missing_members(wanted, &done)
}

fn missing_members(wanted: &[(String, String, PathBuf)], done: &[bool]) -> Result<()> {
    let missing: Vec<&str> = wanted
        .iter()
        .zip(done)
        .filter(|(_, d)| !**d)
        .map(|((m, _, _), _)| m.as_str())
        .collect();
    if missing.is_empty() {
        Ok(())
    } else {
        Err(anyhow!("the archive has no {}", missing.join(", ")))
    }
}

#[cfg(unix)]
fn make_executable(p: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(p, std::fs::Permissions::from_mode(0o755))
        .with_context(|| format!("marking {} executable", p.display()))
}

#[cfg(not(unix))]
fn make_executable(_: &Path) -> Result<()> {
    Ok(())
}

/// One managed tool's line in `bin/tools.json`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Record {
    #[serde(default)]
    pub version: Option<String>,
    /// Unix seconds.
    #[serde(default)]
    pub installed_at: Option<i64>,
    /// Unix seconds of the last update check (yt-dlp only).
    #[serde(default)]
    pub last_check: Option<i64>,
    /// `nightly` / `stable`: which channel the installed yt-dlp came from, so
    /// switching channels triggers an update even inside the 24 h window.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub channel: Option<String>,
}

/// `bin/tools.json`: `{ "ytdlp": {…}, "ffmpeg": {…}, "deno": {…} }`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Manifest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ytdlp: Option<Record>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ffmpeg: Option<Record>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deno: Option<Record>,
}

impl Manifest {
    pub fn get(&self, kind: ToolKind) -> Option<&Record> {
        match kind {
            ToolKind::Ytdlp => self.ytdlp.as_ref(),
            ToolKind::Ffmpeg => self.ffmpeg.as_ref(),
            ToolKind::Deno => self.deno.as_ref(),
        }
    }

    pub fn entry(&mut self, kind: ToolKind) -> &mut Record {
        let slot = match kind {
            ToolKind::Ytdlp => &mut self.ytdlp,
            ToolKind::Ffmpeg => &mut self.ffmpeg,
            ToolKind::Deno => &mut self.deno,
        };
        slot.get_or_insert_with(Record::default)
    }

    /// A missing or unreadable file is an empty manifest: the worst that
    /// costs is one early update check.
    pub fn load(bin: &Path) -> Manifest {
        std::fs::read_to_string(bin.join(MANIFEST))
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    /// Written to a temp name and renamed, so a crash mid-write cannot leave
    /// half a file.
    pub fn save(&self, bin: &Path) -> Result<()> {
        std::fs::create_dir_all(bin)?;
        let tmp = bin.join(format!("{MANIFEST}.tmp"));
        std::fs::write(&tmp, serde_json::to_string_pretty(self)?)?;
        std::fs::rename(&tmp, bin.join(MANIFEST))?;
        Ok(())
    }
}

/// Test support: a one-connection-at-a-time HTTP/1.1 server on localhost that
/// serves fixed bodies, and can cut a body off halfway.
#[cfg(test)]
pub mod testserver {
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    #[derive(Clone)]
    pub enum Reply {
        Body(Vec<u8>),
        /// Announces the full length but sends only the first half, then hangs up.
        Truncated(Vec<u8>),
        /// The whole body, in four pieces 400 ms apart.
        Slow(Vec<u8>),
        Redirect(String),
        NotFound,
    }

    #[derive(Clone, Default)]
    pub struct Routes(pub Arc<Mutex<HashMap<String, Reply>>>);

    impl Routes {
        pub fn set(&self, path: &str, r: Reply) {
            self.0.lock().unwrap().insert(path.to_string(), r);
        }
    }

    /// Starts the server; returns its base URL (`http://127.0.0.1:<port>`).
    pub async fn start(routes: Routes) -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            loop {
                let Ok((mut sock, _)) = listener.accept().await else { return };
                let routes = routes.clone();
                tokio::spawn(async move {
                    let mut buf = Vec::new();
                    let mut tmp = [0u8; 1024];
                    while !buf.windows(4).any(|w| w == b"\r\n\r\n") {
                        match sock.read(&mut tmp).await {
                            Ok(0) | Err(_) => return,
                            Ok(n) => buf.extend_from_slice(&tmp[..n]),
                        }
                    }
                    let req = String::from_utf8_lossy(&buf);
                    let path = req.split_whitespace().nth(1).unwrap_or("/").to_string();
                    let reply = routes.0.lock().unwrap().get(&path).cloned().unwrap_or(Reply::NotFound);
                    let _ = match reply {
                        Reply::Body(b) => {
                            let head = format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", b.len());
                            sock.write_all(head.as_bytes()).await.and(sock.write_all(&b).await)
                        }
                        Reply::Slow(b) => {
                            let head = format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", b.len());
                            let mut r = sock.write_all(head.as_bytes()).await;
                            for piece in b.chunks(b.len().div_ceil(4).max(1)) {
                                tokio::time::sleep(std::time::Duration::from_millis(400)).await;
                                r = r.and(sock.write_all(piece).await).and(sock.flush().await);
                            }
                            r
                        }
                        Reply::Truncated(b) => {
                            let head = format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", b.len());
                            sock.write_all(head.as_bytes()).await.and(sock.write_all(&b[..b.len() / 2]).await)
                        }
                        Reply::Redirect(to) => {
                            let head = format!("HTTP/1.1 302 Found\r\nLocation: {to}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
                            sock.write_all(head.as_bytes()).await
                        }
                        Reply::NotFound => {
                            sock.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").await
                        }
                    };
                    let _ = sock.shutdown().await;
                });
            }
        });
        format!("http://{addr}")
    }
}

#[cfg(test)]
pub(crate) fn sha256_hex(data: &[u8]) -> String {
    hex(&Sha256::digest(data))
}

#[cfg(test)]
mod tests {
    use super::testserver::{start, Reply, Routes};
    use super::*;
    use std::io::Write;
    use std::sync::Mutex;

    fn client() -> reqwest::Client {
        reqwest::Client::builder().timeout(Duration::from_secs(30)).build().unwrap()
    }

    fn no_progress() -> impl Fn(ToolProgress) + Send + Sync {
        |_| {}
    }

    fn bare_plan(base: &str, name: &str) -> Plan {
        Plan {
            kind: ToolKind::Ytdlp,
            version: Some("2026.09.16".into()),
            fetches: vec![Fetch {
                url: format!("{base}/dl/{name}"),
                checksum: Checksum::InList { url: format!("{base}/SUMS"), name: name.into() },
                unpack: Unpack::Bare { dest: "yt-dlp".into() },
            }],
        }
    }

    async fn install(http: &reqwest::Client, bin: &Path, plan: &Plan) -> Result<Vec<PathBuf>> {
        stage(http, bin, plan, &no_progress()).await?.place(bin)
    }

    fn staging_is_empty(bin: &Path) -> bool {
        std::fs::read_dir(bin.join(STAGING)).map(|rd| rd.count() == 0).unwrap_or(true)
    }

    #[tokio::test]
    async fn a_verified_download_replaces_the_old_binary() {
        let routes = Routes::default();
        let base = start(routes.clone()).await;
        let body = b"#!/bin/sh\necho new\n".to_vec();
        routes.set("/SUMS", Reply::Body(format!("{}  yt-dlp_linux\n", sha256_hex(&body)).into_bytes()));
        routes.set("/dl/yt-dlp_linux", Reply::Body(body.clone()));
        let tmp = tempfile::tempdir().unwrap();
        let bin = tmp.path();
        std::fs::write(bin.join("yt-dlp"), b"old").unwrap();

        let placed = install(&client(), bin, &bare_plan(&base, "yt-dlp_linux")).await.unwrap();
        assert_eq!(placed, vec![bin.join("yt-dlp")]);
        assert_eq!(std::fs::read(bin.join("yt-dlp")).unwrap(), body);
        assert!(!bin.join("yt-dlp.old").exists(), "the old copy is removed once unused");
        assert!(staging_is_empty(bin));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(bin.join("yt-dlp")).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o755);
        }
    }

    #[tokio::test]
    async fn a_checksum_mismatch_installs_nothing() {
        let routes = Routes::default();
        let base = start(routes.clone()).await;
        routes.set("/SUMS", Reply::Body(format!("{}  yt-dlp_linux\n", sha256_hex(b"what was published")).into_bytes()));
        routes.set("/dl/yt-dlp_linux", Reply::Body(b"what arrived".to_vec()));
        let tmp = tempfile::tempdir().unwrap();
        let bin = tmp.path();
        std::fs::write(bin.join("yt-dlp"), b"old").unwrap();

        let err = install(&client(), bin, &bare_plan(&base, "yt-dlp_linux")).await.unwrap_err();
        assert!(err.to_string().contains("checksum mismatch"), "{err:#}");
        assert_eq!(std::fs::read(bin.join("yt-dlp")).unwrap(), b"old");
        assert!(staging_is_empty(bin), "no half-file left in staging");
    }

    #[tokio::test]
    async fn a_download_cut_off_halfway_leaves_the_old_binary() {
        let routes = Routes::default();
        let base = start(routes.clone()).await;
        let body = vec![7u8; 64 * 1024];
        routes.set("/SUMS", Reply::Body(format!("{}  yt-dlp_linux\n", sha256_hex(&body)).into_bytes()));
        routes.set("/dl/yt-dlp_linux", Reply::Truncated(body));
        let tmp = tempfile::tempdir().unwrap();
        let bin = tmp.path();
        std::fs::write(bin.join("yt-dlp"), b"old").unwrap();

        assert!(install(&client(), bin, &bare_plan(&base, "yt-dlp_linux")).await.is_err());
        assert_eq!(std::fs::read(bin.join("yt-dlp")).unwrap(), b"old");
        assert!(staging_is_empty(bin));

        // And with no old binary, nothing appears at the final path at all.
        let fresh = tempfile::tempdir().unwrap();
        assert!(install(&client(), fresh.path(), &bare_plan(&base, "yt-dlp_linux")).await.is_err());
        assert!(!fresh.path().join("yt-dlp").exists());
    }

    #[tokio::test]
    async fn a_download_outlasting_the_clients_total_timeout_still_completes() {
        // lib.rs hands Tools a client with a 30 s total timeout; a 150 MB
        // ffmpeg takes longer than that on an ordinary line. Scaled down: a
        // 1 s client, a body that takes ~1.6 s to arrive.
        let routes = Routes::default();
        let base = start(routes.clone()).await;
        let body = vec![5u8; 4096];
        routes.set("/SUMS", Reply::Body(format!("{}  yt-dlp_linux\n", sha256_hex(&body)).into_bytes()));
        routes.set("/dl/yt-dlp_linux", Reply::Slow(body.clone()));
        let impatient = reqwest::Client::builder().timeout(Duration::from_secs(1)).build().unwrap();
        let tmp = tempfile::tempdir().unwrap();
        install(&impatient, tmp.path(), &bare_plan(&base, "yt-dlp_linux")).await.unwrap();
        assert_eq!(std::fs::read(tmp.path().join("yt-dlp")).unwrap(), body);
    }

    #[tokio::test]
    async fn a_checksum_list_without_the_asset_is_an_error_before_downloading() {
        let routes = Routes::default();
        let base = start(routes.clone()).await;
        routes.set("/SUMS", Reply::Body(b"0000  something_else\n".to_vec()));
        // No route for the asset: had it been requested, the error would say 404.
        let tmp = tempfile::tempdir().unwrap();
        let err = install(&client(), tmp.path(), &bare_plan(&base, "yt-dlp_linux")).await.unwrap_err();
        assert!(err.to_string().contains("lists no checksum for yt-dlp_linux"), "{err:#}");
    }

    /// The step that puts the new binary at its final name fails (a full
    /// disk, a scanner holding the file, a permissions surprise). The working
    /// binary must still be there, under its own name, afterwards.
    #[test]
    fn a_failed_final_rename_leaves_the_old_binary_in_place() {
        let tmp = tempfile::tempdir().unwrap();
        let bin = tmp.path();
        let staging = bin.join(STAGING);
        std::fs::create_dir_all(&staging).unwrap();
        let new = staging.join("yt-dlp.new");
        std::fs::write(&new, b"new").unwrap();
        std::fs::write(bin.join("yt-dlp"), b"old").unwrap();
        let staged = Staged { files: vec![(new.clone(), "yt-dlp".into())] };

        let err = staged
            .place_with(bin, &|from, to| {
                if from == new.as_path() {
                    Err(io::Error::other("simulated: the final rename failed"))
                } else {
                    std::fs::rename(from, to)
                }
            })
            .unwrap_err();
        assert!(format!("{err:#}").contains("simulated"), "{err:#}");
        assert_eq!(std::fs::read(bin.join("yt-dlp")).unwrap(), b"old");
        let mut names: Vec<String> = std::fs::read_dir(bin)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|n| n != STAGING)
            .collect();
        names.sort();
        assert_eq!(names, vec!["yt-dlp"], "no .old left beside it");
        assert!(staging_is_empty(bin), "the unplaced file is not left in staging");
    }

    /// The same, for real: the destination is a non-empty directory, which no
    /// rename can replace.
    #[cfg(unix)]
    #[test]
    fn a_destination_no_rename_can_replace_is_an_error_and_touches_nothing_else() {
        let tmp = tempfile::tempdir().unwrap();
        let bin = tmp.path();
        std::fs::create_dir_all(bin.join(STAGING)).unwrap();
        let new_ff = bin.join(STAGING).join("ffmpeg.new");
        let new_probe = bin.join(STAGING).join("ffprobe.new");
        std::fs::write(&new_ff, b"new ffmpeg").unwrap();
        std::fs::write(&new_probe, b"new ffprobe").unwrap();
        std::fs::write(bin.join("ffmpeg"), b"old ffmpeg").unwrap();
        std::fs::create_dir_all(bin.join("ffprobe").join("in the way")).unwrap();
        let staged = Staged {
            files: vec![(new_ff, "ffmpeg".into()), (new_probe, "ffprobe".into())],
        };
        assert!(staged.place(bin).is_err());
        // ffmpeg was replaced whole before ffprobe failed; nothing was ever
        // missing, and nothing is left staged.
        assert_eq!(std::fs::read(bin.join("ffmpeg")).unwrap(), b"new ffmpeg");
        assert!(bin.join("ffprobe").join("in the way").is_dir());
        assert!(staging_is_empty(bin));
    }

    #[tokio::test]
    async fn an_interrupted_install_is_swept_at_the_next_start() {
        let tmp = tempfile::tempdir().unwrap();
        let bin = tmp.path();
        std::fs::create_dir_all(bin.join(STAGING)).unwrap();
        std::fs::write(bin.join(STAGING).join("yt-dlp-0-yt-dlp_linux.part"), b"half").unwrap();
        std::fs::write(bin.join("yt-dlp.old"), b"previous").unwrap();
        std::fs::write(bin.join("deno.1789931890.old"), b"previous").unwrap();
        std::fs::write(bin.join("yt-dlp"), b"current").unwrap();
        sweep(bin);
        assert!(!bin.join(STAGING).exists());
        assert!(!bin.join("yt-dlp.old").exists());
        assert!(!bin.join("deno.1789931890.old").exists());
        assert_eq!(std::fs::read(bin.join("yt-dlp")).unwrap(), b"current");
    }

    fn zip_bytes(members: &[(&str, &[u8])]) -> Vec<u8> {
        let mut buf = io::Cursor::new(Vec::new());
        {
            let mut w = zip::ZipWriter::new(&mut buf);
            let opts = zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Deflated);
            for (name, data) in members {
                w.start_file(*name, opts).unwrap();
                w.write_all(data).unwrap();
            }
            w.finish().unwrap();
        }
        buf.into_inner()
    }

    fn tar_xz_bytes(members: &[(&str, &[u8])]) -> Vec<u8> {
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

    fn ffmpeg_take() -> Vec<(String, String)> {
        vec![("ffmpeg".into(), "ffmpeg".into()), ("ffprobe".into(), "ffprobe".into())]
    }

    #[tokio::test]
    async fn only_the_named_executables_come_out_of_a_tar_xz() {
        let routes = Routes::default();
        let base = start(routes.clone()).await;
        let archive = tar_xz_bytes(&[
            ("ffmpeg-master-latest-linux64-gpl/LICENSE.txt", b"GPL"),
            ("ffmpeg-master-latest-linux64-gpl/bin/ffmpeg", b"FFMPEG"),
            ("ffmpeg-master-latest-linux64-gpl/bin/ffplay", b"FFPLAY"),
            ("ffmpeg-master-latest-linux64-gpl/bin/ffprobe", b"FFPROBE"),
            ("ffmpeg-master-latest-linux64-gpl/doc/ffmpeg.html", b"<html>"),
        ]);
        let name = "ffmpeg-master-latest-linux64-gpl.tar.xz";
        routes.set("/checksums.sha256", Reply::Body(format!("{}  {name}\n", sha256_hex(&archive)).into_bytes()));
        routes.set(&format!("/{name}"), Reply::Body(archive));
        let plan = Plan {
            kind: ToolKind::Ffmpeg,
            version: None,
            fetches: vec![Fetch {
                url: format!("{base}/{name}"),
                checksum: Checksum::InList { url: format!("{base}/checksums.sha256"), name: name.into() },
                unpack: Unpack::TarXz { take: ffmpeg_take() },
            }],
        };
        let tmp = tempfile::tempdir().unwrap();
        let bin = tmp.path();
        let seen = Mutex::new(Vec::new());
        let staged = stage(&client(), bin, &plan, &|p: ToolProgress| seen.lock().unwrap().push(p.phase)).await.unwrap();
        staged.place(bin).unwrap();
        assert_eq!(std::fs::read(bin.join("ffmpeg")).unwrap(), b"FFMPEG");
        assert_eq!(std::fs::read(bin.join("ffprobe")).unwrap(), b"FFPROBE");
        let mut names: Vec<String> = std::fs::read_dir(bin)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|n| n != STAGING)
            .collect();
        names.sort();
        assert_eq!(names, vec!["ffmpeg", "ffprobe"]);
        assert!(staging_is_empty(bin));
        let seen = seen.lock().unwrap();
        assert!(seen.contains(&ToolPhase::Download) && seen.contains(&ToolPhase::Extract), "{seen:?}");
    }

    #[tokio::test]
    async fn a_zip_missing_a_wanted_member_installs_nothing() {
        let routes = Routes::default();
        let base = start(routes.clone()).await;
        let archive = zip_bytes(&[("ffmpeg-master-latest-win64-gpl/bin/ffmpeg.exe", b"FFMPEG")]);
        routes.set("/sums", Reply::Body(format!("{}  a.zip\n", sha256_hex(&archive)).into_bytes()));
        routes.set("/a.zip", Reply::Body(archive));
        let plan = Plan {
            kind: ToolKind::Ffmpeg,
            version: None,
            fetches: vec![Fetch {
                url: format!("{base}/a.zip"),
                checksum: Checksum::InList { url: format!("{base}/sums"), name: "a.zip".into() },
                unpack: Unpack::Zip {
                    take: vec![
                        ("ffmpeg.exe".into(), "ffmpeg.exe".into()),
                        ("ffprobe.exe".into(), "ffprobe.exe".into()),
                    ],
                },
            }],
        };
        let tmp = tempfile::tempdir().unwrap();
        let err = install(&client(), tmp.path(), &plan).await.unwrap_err();
        assert!(err.to_string().contains("ffprobe.exe"), "{err:#}");
        assert!(!tmp.path().join("ffmpeg.exe").exists());
        assert!(staging_is_empty(tmp.path()));
    }

    #[tokio::test]
    async fn a_martin_riedl_style_redirect_is_verified_beside_where_it_landed() {
        let routes = Routes::default();
        let base = start(routes.clone()).await;
        let ffmpeg = zip_bytes(&[("ffmpeg", b"FFMPEG")]);
        let ffprobe = zip_bytes(&[("ffprobe", b"FFPROBE")]);
        for (tool, data) in [("ffmpeg", &ffmpeg), ("ffprobe", &ffprobe)] {
            routes.set(
                &format!("/redirect/latest/macos/arm64/release/{tool}.zip"),
                Reply::Redirect(format!("/download/macos/arm64/1789931890_9.0.2/{tool}.zip")),
            );
            routes.set(&format!("/download/macos/arm64/1789931890_9.0.2/{tool}.zip"), Reply::Body(data.clone()));
            routes.set(
                &format!("/download/macos/arm64/1789931890_9.0.2/{tool}.zip.sha256"),
                Reply::Body(format!("{}  {tool}.zip\n", sha256_hex(data)).into_bytes()),
            );
        }
        let fetch = |tool: &str| Fetch {
            url: format!("{base}/redirect/latest/macos/arm64/release/{tool}.zip"),
            checksum: Checksum::BesideFinal { suffix: ".sha256" },
            unpack: Unpack::Zip { take: vec![(tool.into(), tool.into())] },
        };
        let plan = Plan { kind: ToolKind::Ffmpeg, version: None, fetches: vec![fetch("ffmpeg"), fetch("ffprobe")] };
        let tmp = tempfile::tempdir().unwrap();
        install(&client(), tmp.path(), &plan).await.unwrap();
        assert_eq!(std::fs::read(tmp.path().join("ffmpeg")).unwrap(), b"FFMPEG");
        assert_eq!(std::fs::read(tmp.path().join("ffprobe")).unwrap(), b"FFPROBE");

        // A wrong hash on the second of two fetches leaves neither behind.
        routes.set(
            "/download/macos/arm64/1789931890_9.0.2/ffprobe.zip.sha256",
            Reply::Body(format!("{}  ffprobe.zip\n", sha256_hex(b"other")).into_bytes()),
        );
        let fresh = tempfile::tempdir().unwrap();
        assert!(install(&client(), fresh.path(), &plan).await.is_err());
        assert!(!fresh.path().join("ffmpeg").exists());
        assert!(!fresh.path().join("ffprobe").exists());
        assert!(staging_is_empty(fresh.path()));
    }

    #[test]
    fn the_manifest_round_trips_in_its_documented_shape() {
        let tmp = tempfile::tempdir().unwrap();
        assert_eq!(Manifest::load(tmp.path()), Manifest::default());
        let mut m = Manifest::default();
        *m.entry(ToolKind::Ytdlp) = Record {
            version: Some("2026.09.16.232951".into()),
            installed_at: Some(1_790_000_000),
            last_check: Some(1_790_000_100),
            channel: Some("nightly".into()),
        };
        m.entry(ToolKind::Deno).version = Some("2.9.7".into());
        m.save(tmp.path()).unwrap();
        assert_eq!(Manifest::load(tmp.path()), m);

        let raw: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(tmp.path().join(MANIFEST)).unwrap()).unwrap();
        assert_eq!(raw["ytdlp"]["version"], "2026.09.16.232951");
        assert_eq!(raw["ytdlp"]["installed_at"], 1_790_000_000);
        assert_eq!(raw["ytdlp"]["last_check"], 1_790_000_100);
        assert_eq!(raw["ytdlp"]["channel"], "nightly");
        assert!(raw.get("ffmpeg").is_none());
        assert!(!tmp.path().join(format!("{MANIFEST}.tmp")).exists());

        std::fs::write(tmp.path().join(MANIFEST), "{ not json").unwrap();
        assert_eq!(Manifest::load(tmp.path()), Manifest::default());
    }

    #[test]
    fn asset_names_are_safe_file_names() {
        assert_eq!(asset_name("https://x/releases/download/v2.9.7/deno-x86_64-unknown-linux-gnu.zip"), "deno-x86_64-unknown-linux-gnu.zip");
        assert_eq!(asset_name("https://x/a/ffmpeg.zip?sig=1"), "ffmpeg.zip");
        assert_eq!(asset_name("https://x/"), "download");
    }
}
