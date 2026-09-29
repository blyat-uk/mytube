use anyhow::{anyhow, Result};
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::models::{FlatEntry, ProbeInfo, VideoStatus};
use crate::proc;
use crate::quality::{self, Quality};
use crate::tools::Invocation;

pub const PROGRESS_PREFIX: &str = "MYTUBE|";
const PROGRESS_TEMPLATE: &str =
    "MYTUBE|%(progress._percent_str)s|%(progress._speed_str)s|%(progress._eta_str)s";

/// Where yt-dlp gets YouTube cookies from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Cookies {
    None,
    /// A `--cookies-from-browser` spec, verbatim (`firefox`, `chrome:Profile 1`).
    Browser(String),
    /// A Netscape cookies.txt, passed as `--cookies`.
    File(PathBuf),
}

/// Everything one yt-dlp run needs to know about this machine: which binary,
/// where ffmpeg and deno are, and whose cookies to present. Built by
/// `tools::Tools::ytdlp`.
#[derive(Debug, Clone)]
pub struct Runner {
    pub program: PathBuf,
    /// `--ffmpeg-location`: the directory holding ffmpeg and ffprobe, or --
    /// for an `ffmpeg_path` override -- the ffmpeg binary itself, which yt-dlp
    /// accepts too ("either the path to the binary or its containing
    /// directory"). A binary path is what lets an override name a file that is
    /// not called `ffmpeg`: yt-dlp then runs that file, and looks for ffprobe
    /// beside it under the same name with `ffmpeg` swapped for `ffprobe`.
    pub ffmpeg_location: Option<PathBuf>,
    /// `--js-runtimes deno:<path>`.
    pub deno: Option<PathBuf>,
    pub cookies: Cookies,
}

pub fn watch_url(video_id: &str) -> String {
    format!("https://www.youtube.com/watch?v={video_id}")
}

pub fn channel_videos_url(channel_id: &str) -> String {
    format!("https://www.youtube.com/channel/{channel_id}/videos")
}

pub fn thumb_url_for(video_id: &str) -> String {
    format!("https://i.ytimg.com/vi/{video_id}/hqdefault.jpg")
}

fn s(x: &str) -> String { x.to_string() }

/// What every yt-dlp run is told about this machine: how to encode its
/// output (see [`encoding_args`]), and where ffmpeg and deno are, so it uses exactly what MyTube resolved rather than searching `PATH`
/// for itself (a GUI launch on macOS or Windows sees a much shorter one).
/// Each is left out when it resolved to nothing, and yt-dlp then searches on
/// its own.
fn runtime_args(r: &Runner) -> Vec<String> {
    let mut a = encoding_args();
    if let Some(loc) = &r.ffmpeg_location {
        a.extend([s("--ffmpeg-location"), loc.to_string_lossy().into_owned()]);
    }
    if let Some(deno) = &r.deno {
        // `RUNTIME:PATH`, where PATH may be the binary or its directory. deno
        // is the one runtime yt-dlp enables by default, so naming it here only
        // pins *which* deno -- it switches nothing else off.
        a.extend([s("--js-runtimes"), format!("deno:{}", deno.to_string_lossy())]);
    }
    a
}

/// How yt-dlp is to encode what it writes to our pipes.
///
/// On Windows, yt-dlp's `write_string` encodes stdout and stderr with the
/// stream's own encoding, and Python gives a *pipe* the ANSI code page
/// (cp1252 on most Western installs) -- with `errors='ignore'`, so anything
/// outside it is not even mangled but silently dropped: a Japanese title
/// arrives as an empty line, and a probed path loses the same characters --
/// and is then forced back on the download as its `-o`. `--encoding` is the value that function takes before the stream's
/// (checked in yt-dlp 2026.08.19, `utils/_utils.py` and
/// `YoutubeDL._write_string`), and it covers `--print`, `-j`, progress and
/// error lines alike. It changes nothing about filenames on disk.
///
/// Off Windows the locale is UTF-8 in practice, and the flag is left out so
/// the argv stays what it has always been. `--print-to-file` needs nothing:
/// yt-dlp opens that file with `encoding='utf-8'` whatever the platform
/// (see `run_one`'s read of it).
fn encoding_args() -> Vec<String> {
    if cfg!(windows) {
        vec![s("--encoding"), s("utf-8")]
    } else {
        Vec::new()
    }
}

/// The cookie flag, if any. No cookie source means no flag at all: a
/// `--cookies-from-browser firefox` on a machine with no Firefox profile fails
/// every run outright, where no cookies only costs the videos that need them.
fn cookie_args(c: &Cookies) -> Vec<String> {
    match c {
        Cookies::None => Vec::new(),
        Cookies::Browser(spec) => vec![s("--cookies-from-browser"), spec.clone()],
        Cookies::File(path) => vec![s("--cookies"), path.to_string_lossy().into_owned()],
    }
}

/// Flags shared by the probe and the real download, so the probe resolves the
/// same extension the download will actually produce. The format selection is
/// `q`'s (see `quality::format_args`); `Quality::default()` gives the
/// `-f bv*+ba/b --merge-output-format mkv` every download had before it.
pub fn common_format_args(r: &Runner, q: &Quality) -> Vec<String> {
    let mut a = quality::format_args(q);
    a.extend([
        s("--no-playlist"),
        s("--color"), s("no_color"),
    ]);
    a.extend(cookie_args(&r.cookies));
    a.extend(runtime_args(r));
    a.extend([
        s("--remote-components"), s("ejs:npm"),
        s("--remote-components"), s("ejs:github"),
    ]);
    // Off Windows, keep the characters Windows forbids (`:` `?` `|` ...) rather
    // than swapping them for lookalikes: the file is named for the filesystem
    // it is actually written to. On Windows the substitution is what makes the
    // name legal at all.
    #[cfg(not(windows))]
    a.push(s("--no-windows-filenames"));
    a
}

/// Phase 1. Resolves the output template to a concrete path and returns metadata.
/// The `--print` order is load-bearing: `parse_probe_output` reads lines positionally.
pub fn probe_args(r: &Runner, q: &Quality, url: &str, download_dir: &str,
                  filename_template: &str) -> Vec<String> {
    let mut a = common_format_args(r, q);
    a.extend([
        s("--simulate"), s("--no-warnings"),
        s("--print"), s("%(id)s"),
        s("--print"), s("%(title)s"),
        s("--print"), s("%(duration)s"),
        s("--print"), s("%(timestamp)s"),
        s("--print"), s("%(channel)s"),
        s("--print"), s("%(channel_id)s"),
        s("--print"), s("filename"),
        s("-P"), s(download_dir),
        s("-o"), s(filename_template),
        s(url),
    ]);
    a
}

fn opt_field(v: &str) -> Option<&str> {
    let t = v.trim();
    if t.is_empty() || t == "NA" { None } else { Some(t) }
}

pub fn parse_probe_output(stdout: &str) -> Result<ProbeInfo> {
    let lines: Vec<&str> = stdout.lines().collect();
    if lines.len() < 7 {
        return Err(anyhow!("yt-dlp probe returned {} lines, expected 7", lines.len()));
    }
    // Take the LAST seven lines: warnings may precede them on some inputs.
    let l = &lines[lines.len() - 7..];
    Ok(ProbeInfo {
        id: l[0].trim().to_string(),
        // Trimmed like every other field: surrounding whitespace is never part
        // of a title, and a trailing \r must not survive into the database.
        title: l[1].trim().to_string(),
        duration_secs: opt_field(l[2]).and_then(|x| x.parse::<f64>().ok()).map(|d| d as i64),
        published_at: opt_field(l[3]).and_then(|x| x.parse::<i64>().ok()),
        channel_title: opt_field(l[4]).unwrap_or_default().to_string(),
        channel_id: opt_field(l[5]).unwrap_or_default().to_string(),
        intended_path: l[6].trim().to_string(),
    })
}

/// `-o` is a template, so a literal `%` in a path must be doubled.
/// Verified: `-o '/tmp/Weird 100%% name (2).%(ext)s'` produced `Weird 100% name (2).mkv`.
pub fn escape_out_template(path: &Path) -> String {
    path.to_string_lossy().replace('%', "%%")
}

/// Finds a free path by inserting ` (N)` before the extension, N starting at 2.
/// `is_taken` covers both files on disk and paths already claimed by in-flight jobs.
pub fn unique_path(intended: &Path, is_taken: impl Fn(&Path) -> bool) -> PathBuf {
    if !is_taken(intended) { return intended.to_path_buf(); }

    let parent = intended.parent().map(Path::to_path_buf).unwrap_or_default();
    let name = intended.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    // Split on the LAST dot only, so dots inside the stem survive.
    let (stem, ext) = match name.rfind('.') {
        Some(i) if i > 0 => (name[..i].to_string(), name[i..].to_string()),
        _ => (name.clone(), String::new()),
    };

    for n in 2..10_000 {
        let candidate = parent.join(format!("{stem} ({n}){ext}"));
        if !is_taken(&candidate) { return candidate; }
    }
    // Practically unreachable; keeps the signature total.
    parent.join(format!("{stem} ({}){ext}", chrono::Utc::now().timestamp()))
}

/// Phase 2. The output path is already decided, so it is forced as an absolute
/// `-o`, which overrides `-P` (verified). yt-dlp never picks the final name.
///
/// The thumbnail is embedded only when `quality::final_ext` names the
/// container, because yt-dlp cannot embed into webm and fails the *whole*
/// download when asked to -- after the file is fully written. Verified
/// 2026-09-29 (yt-dlp 2026.09.27, jNQXAC9IVRw): `-f 251 --embed-thumbnail`
/// and `-f bv[ext=webm] --embed-thumbnail` both exit 1 with `ERROR:
/// Postprocessing: Supported filetypes for thumbnail embedding are: mp3,
/// mkv/mka, ogg/opus/flac, m4a/mp4/m4v/mov`. An `original`-audio pick or a
/// single-stream raw format may well be webm, and `final_ext` is `None` for
/// exactly those.
pub fn download_args(r: &Runner, q: &Quality, video_id: &str, out_path: &Path,
                     print_file: &Path) -> Vec<String> {
    let mut a = common_format_args(r, q);
    if quality::final_ext(q).is_some() {
        a.extend([
            s("--embed-thumbnail"),
            s("--convert-thumbnails"), s("jpg"),
        ]);
    }
    a.extend([
        s("--newline"),
        s("--progress-template"), s(PROGRESS_TEMPLATE),
        s("--print-to-file"), s("after_move:filepath"),
        print_file.to_string_lossy().into_owned(),
        s("-o"), escape_out_template(out_path),
        watch_url(video_id),
    ]);
    a
}

/// Runs phase 1 and returns the resolved metadata plus intended path.
///
/// Whether a listing error is YouTube refusing a channel whose account it has
/// terminated. The whole of what yt-dlp prints, seen on a real subscription:
/// `ERROR: [youtube:tab] UC…: YouTube said: This account has been terminated
/// for violating YouTube or Google's Terms of Service.` The reason after "for"
/// varies (copyright strikes, spam, …), so only the fixed part is matched.
pub fn is_terminated(listing_error: &str) -> bool {
    listing_error.contains("This account has been terminated")
}

/// The caller holds `inv` for as long as this runs, which is what keeps the
/// updater from swapping yt-dlp out from under it.
pub async fn probe(inv: &Invocation, q: &Quality, url: &str, download_dir: &str,
                   filename_template: &str) -> Result<ProbeInfo> {
    let r = &inv.runner;
    let mut cmd = proc::command(&r.program);
    cmd.args(probe_args(r, q, url, download_dir, filename_template));
    // Cancelling during phase 1 aborts the task, which drops this future.
    // Without a kill on drop that leaves a yt-dlp behind with nothing left to
    // reap it -- the same trap `run_with_timeout` documents below -- and it has
    // to be the whole tree: `kill_on_drop` alone reaches PyInstaller's
    // bootloader and not the real yt-dlp it runs as a child.
    let out = proc::output(&mut cmd).await
        .map_err(|e| anyhow!("could not run yt-dlp: {e}"))?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        return Err(anyhow!("{}", err.lines().rev().find(|l| l.contains("ERROR"))
            .unwrap_or("yt-dlp could not read that video")));
    }
    parse_probe_output(&String::from_utf8_lossy(&out.stdout))
}

/// JSON lines, never `--print` with a `|` separator: titles contain `|`.
///
/// No cookies, deliberately. The listing serves members-only entries to anyone
/// (see `poll::ingest_listing`), so cookies would buy it nothing, and reading
/// them is not free: it decrypts a browser's cookie store for every channel of
/// every poll, and on macOS a Chromium-family browser asks for Keychain access
/// each time it does.
pub fn flat_playlist_args(r: &Runner, channel_id: &str, limit: u32) -> Vec<String> {
    let mut a = runtime_args(r);
    a.extend([
        s("--flat-playlist"),
        s("-j"),
        // Without this, a flat listing carries no upload date at all, and
        // backfilled videos cannot be interleaved into the feed by date.
        // Day-granular is plenty for ordering; RSS later supplies exact times
        // for the recent window.
        s("--extractor-args"), s("youtubetab:approximate_date"),
        s("--playlist-end"), limit.to_string(),
        s("--no-warnings"),
        s("--color"), s("no_color"),
        channel_videos_url(channel_id),
    ]);
    a
}

pub fn parse_progress_line(line: &str) -> Option<(f64, String, String)> {
    let rest = line.trim().strip_prefix(PROGRESS_PREFIX)?;
    let parts: Vec<&str> = rest.split('|').collect();
    if parts.len() != 3 { return None; }
    let percent = parts[0].trim().trim_end_matches('%').trim().parse::<f64>().ok()?;
    if !percent.is_finite() { return None; }
    Some((percent.clamp(0.0, 100.0), parts[1].trim().to_string(), parts[2].trim().to_string()))
}

pub fn parse_flat_entry(json_line: &str) -> Option<FlatEntry> {
    let v: serde_json::Value = serde_json::from_str(json_line.trim()).ok()?;
    let id = v.get("id")?.as_str()?.to_string();
    Some(FlatEntry {
        id,
        title: v.get("title").and_then(|x| x.as_str()).unwrap_or_default().to_string(),
        duration_secs: v.get("duration").and_then(|x| x.as_f64()).map(|d| d as i64),
        live_status: v.get("live_status").and_then(|x| x.as_str()).map(str::to_string),
        view_count: v.get("view_count").and_then(|x| x.as_i64()),
        availability: v.get("availability").and_then(|x| x.as_str()).map(str::to_string),
        published_at: v
            .get("timestamp")
            .and_then(|x| x.as_i64())
            .or_else(|| v.get("upload_date").and_then(|x| x.as_str()).and_then(parse_upload_date)),
    })
}

/// `upload_date` is `YYYYMMDD`; used only when `timestamp` is absent.
fn parse_upload_date(d: &str) -> Option<i64> {
    if d.len() != 8 { return None; }
    let y: i32 = d[0..4].parse().ok()?;
    let m: u32 = d[4..6].parse().ok()?;
    let day: u32 = d[6..8].parse().ok()?;
    chrono::NaiveDate::from_ymd_opt(y, m, day)?
        .and_hms_opt(0, 0, 0)
        .map(|dt| dt.and_utc().timestamp())
}

pub fn status_from(live_status: Option<&str>, duration: Option<i64>) -> VideoStatus {
    match live_status {
        Some("is_live") => VideoStatus::Live,
        Some("is_upcoming") => VideoStatus::Upcoming,
        _ => match duration {
            Some(d) if d > 0 => VideoStatus::Ready,
            _ => VideoStatus::Pending,
        },
    }
}

/// How long a channel listing may take before it is written off.
///
/// Generous: a 200-entry listing normally lands in a few seconds, and the only
/// job of this number is to be finite.
const LISTING_TIMEOUT: Duration = Duration::from_secs(180);

/// Runs a child and captures its output, with a ceiling on how long it may
/// take.
///
/// `.output()` on its own waits forever. That is survivable for a download,
/// which you can see sitting there and cancel, but `flat_playlist` runs inside
/// the poll loop: one wedged child stops *every* future poll for the life of
/// the process, because the loop polls sequentially and holds `poll_lock`
/// while it does -- so a manual refresh then answers "A refresh is already
/// running" and only a restart clears it. Nothing is on screen to say so
/// either, when the window is closed to the tray.
///
/// Killing the child on drop is what makes the ceiling real: dropping the
/// future on timeout otherwise leaves the child running with nothing left to
/// reap it. `proc::output` kills its whole tree, because `kill_on_drop` alone
/// reaches only PyInstaller's bootloader and would leave the real yt-dlp it
/// runs as a child wedged exactly as before -- one more per poll interval.
async fn run_with_timeout(
    program: &Path,
    args: Vec<String>,
    limit: Duration,
) -> Result<std::process::Output> {
    let mut cmd = proc::command(program);
    cmd.args(args);
    let name = program.display();
    match tokio::time::timeout(limit, proc::output(&mut cmd)).await {
        Ok(finished) => finished.map_err(|e| anyhow!("could not run {name}: {e}")),
        Err(_) => Err(anyhow!("{name} timed out after {}s", limit.as_secs())),
    }
}

/// The caller holds `inv` for as long as this runs, which is what keeps the
/// updater from swapping yt-dlp out from under it.
pub async fn flat_playlist(inv: &Invocation, channel_id: &str, limit: u32)
    -> Result<Vec<FlatEntry>> {
    let r = &inv.runner;
    let out = run_with_timeout(&r.program, flat_playlist_args(r, channel_id, limit),
                               LISTING_TIMEOUT)
        .await?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        return Err(anyhow!("yt-dlp listing failed for {channel_id}: {}",
                           err.lines().last().unwrap_or("unknown error")));
    }
    Ok(String::from_utf8_lossy(&out.stdout)
        .lines().filter_map(parse_flat_entry).collect())
}

// ------------------------------------------------------------- formats

/// Everything the "Download (custom)…" dialog can offer for one video, and
/// what the Settings default would pick from it. A dialog payload, so
/// camelCase, like `ArchiveSummary`.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VideoFormats {
    pub title: String,
    /// Height descending, then size descending.
    pub video: Vec<VideoTrack>,
    /// Bitrate descending.
    pub audio: Vec<AudioTrack>,
    /// The format ids the Settings default resolves to, when they are among
    /// the tracks listed -- a pre-muxed pick (`18`) is not, and leaves both
    /// `None`, since there is no row for the dialog to pre-select.
    pub default_video: Option<String>,
    pub default_audio: Option<String>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VideoTrack {
    pub id: String,
    pub height: u32,
    pub fps: Option<f64>,
    /// `av1`, `vp9`, `h264` or `other`.
    pub vcodec: &'static str,
    /// yt-dlp's raw vcodec string, `avc1.640028`.
    pub codec: String,
    pub hdr: bool,
    pub ext: String,
    /// Bytes; yt-dlp's `filesize`, else its `filesize_approx`.
    pub size: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AudioTrack {
    pub id: String,
    /// `opus`, `aac` or `other`.
    pub acodec: &'static str,
    pub codec: String,
    pub abr: Option<f64>,
    pub language: Option<String>,
    pub ext: String,
    pub size: Option<u64>,
}

/// `-J` for one video, with the same format flags a download of it would get
/// -- so `requested_formats` in the answer is what the Settings default would
/// pick -- and the same cookies, since a members-only video lists nothing
/// without them.
pub fn formats_args(r: &Runner, q: &Quality, video_id: &str) -> Vec<String> {
    let mut a = common_format_args(r, q);
    a.extend([s("-J"), s("--no-warnings"), watch_url(video_id)]);
    a
}

fn str_of<'a>(v: &'a serde_json::Value, key: &str) -> Option<&'a str> {
    v.get(key).and_then(|x| x.as_str())
}

/// `"none"` is how yt-dlp spells "this stream has no video" (or audio); a
/// missing or null codec is unknown, which is not the same thing.
fn is_none_codec(v: &serde_json::Value, key: &str) -> bool {
    str_of(v, key) == Some("none")
}

fn size_of(v: &serde_json::Value) -> Option<u64> {
    ["filesize", "filesize_approx"].iter()
        .find_map(|k| v.get(*k).and_then(|x| x.as_f64()))
        .filter(|n| *n > 0.0)
        .map(|n| n as u64)
}

fn is_m3u8(v: &serde_json::Value) -> bool {
    str_of(v, "protocol").is_some_and(|p| p.starts_with("m3u8"))
}

fn vcodec_family(codec: &str) -> &'static str {
    let c = codec.to_ascii_lowercase();
    if c.starts_with("av01") || c == "av1" {
        "av1"
    } else if c.starts_with("vp9") || c.starts_with("vp09") {
        "vp9"
    } else if c.starts_with("avc") || c.starts_with("h264") {
        "h264"
    } else {
        "other"
    }
}

fn acodec_family(codec: &str) -> &'static str {
    let c = codec.to_ascii_lowercase();
    if c == "opus" {
        "opus"
    } else if c.starts_with("mp4a") || c == "aac" {
        "aac"
    } else {
        "other"
    }
}

/// Keeps the non-HLS entries when there are any, else all of them: HLS
/// duplicates the DASH formats where both exist, and is all there is where
/// they do not (some live replays).
fn prefer_non_m3u8(all: Vec<&serde_json::Value>) -> Vec<&serde_json::Value> {
    if all.iter().any(|f| !is_m3u8(f)) {
        all.into_iter().filter(|f| !is_m3u8(f)).collect()
    } else {
        all
    }
}

/// Reads a `yt-dlp -J` dump. Video-only and audio-only streams are kept --
/// the dialog combines them itself -- and storyboards (`mhtml`) and pre-muxed
/// formats dropped.
pub fn parse_formats(json: &str) -> Result<VideoFormats> {
    let v: serde_json::Value = serde_json::from_str(json.trim())
        .map_err(|e| anyhow!("yt-dlp's format list is not JSON: {e}"))?;
    let empty = Vec::new();
    let formats = v.get("formats").and_then(|f| f.as_array()).unwrap_or(&empty);
    let usable = |f: &&serde_json::Value| {
        str_of(f, "ext") != Some("mhtml") && str_of(f, "format_id").is_some()
    };

    let video_raw: Vec<&serde_json::Value> = formats.iter().filter(usable)
        .filter(|f| is_none_codec(f, "acodec") && !is_none_codec(f, "vcodec"))
        .filter(|f| f.get("height").and_then(|h| h.as_u64()).is_some_and(|h| h > 0))
        .collect();
    let audio_raw: Vec<&serde_json::Value> = formats.iter().filter(usable)
        .filter(|f| is_none_codec(f, "vcodec") && !is_none_codec(f, "acodec"))
        .collect();

    let mut video: Vec<VideoTrack> = prefer_non_m3u8(video_raw).into_iter().map(|f| {
        let codec = str_of(f, "vcodec").unwrap_or_default().to_string();
        VideoTrack {
            id: str_of(f, "format_id").unwrap_or_default().to_string(),
            height: f.get("height").and_then(|h| h.as_u64()).unwrap_or(0) as u32,
            fps: f.get("fps").and_then(|x| x.as_f64()).filter(|x| *x > 0.0),
            vcodec: vcodec_family(&codec),
            codec,
            hdr: str_of(f, "dynamic_range").is_some_and(|d| !d.eq_ignore_ascii_case("SDR")),
            ext: str_of(f, "ext").unwrap_or_default().to_string(),
            size: size_of(f),
        }
    }).collect();
    video.sort_by(|a, b| b.height.cmp(&a.height).then(b.size.cmp(&a.size)));

    let mut audio: Vec<AudioTrack> = prefer_non_m3u8(audio_raw).into_iter().map(|f| {
        let codec = str_of(f, "acodec").unwrap_or_default().to_string();
        AudioTrack {
            id: str_of(f, "format_id").unwrap_or_default().to_string(),
            acodec: acodec_family(&codec),
            codec,
            abr: f.get("abr").and_then(|x| x.as_f64()).filter(|x| *x > 0.0),
            language: str_of(f, "language").filter(|l| !l.is_empty()).map(str::to_string),
            ext: str_of(f, "ext").unwrap_or_default().to_string(),
            size: size_of(f),
        }
    }).collect();
    audio.sort_by(|a, b| b.abr.unwrap_or(0.0).total_cmp(&a.abr.unwrap_or(0.0)));

    // What the default picked: `requested_formats` for a merge, else the one
    // top-level `format_id`.
    let picked: Vec<String> = match v.get("requested_formats").and_then(|r| r.as_array()) {
        Some(r) if !r.is_empty() => r.iter()
            .filter_map(|f| str_of(f, "format_id").map(str::to_string)).collect(),
        _ => str_of(&v, "format_id").map(|id| vec![id.to_string()]).unwrap_or_default(),
    };
    let default_video = picked.iter().find(|id| video.iter().any(|t| &t.id == *id)).cloned();
    let default_audio = picked.iter().find(|id| audio.iter().any(|t| &t.id == *id)).cloned();

    Ok(VideoFormats {
        title: str_of(&v, "title").unwrap_or_default().to_string(),
        video,
        audio,
        default_video,
        default_audio,
    })
}

/// How long `formats` may take. A `-J` is one extraction, a few seconds; the
/// ceiling only has to be finite, since a dialog is waiting on it.
const FORMATS_TIMEOUT: Duration = Duration::from_secs(120);

/// Lists one video's formats. The caller holds `inv` for as long as this runs,
/// which is what keeps the updater from swapping yt-dlp out from under it; a
/// timeout kills the whole tree (see `run_with_timeout`).
pub async fn formats(inv: &Invocation, video_id: &str, q: &Quality) -> Result<VideoFormats> {
    let r = &inv.runner;
    let out = run_with_timeout(&r.program, formats_args(r, q, video_id), FORMATS_TIMEOUT).await?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        return Err(anyhow!("{}", err.lines().rev().find(|l| l.contains("ERROR"))
            .unwrap_or("yt-dlp could not list that video's formats")));
    }
    parse_formats(&String::from_utf8_lossy(&out.stdout))
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_terminated_account_is_recognised_by_what_youtube_said() {
        let seen = "yt-dlp listing failed for UCUHMlS8-7okK-TVBcHDB5Yg: ERROR: [youtube:tab] \
                    UCUHMlS8-7okK-TVBcHDB5Yg: YouTube said: This account has been terminated \
                    for violating YouTube or Google's Terms of Service.";
        assert!(super::is_terminated(seen));
        assert!(super::is_terminated(
            "YouTube said: This account has been terminated due to multiple third-party \
             notifications of copyright infringement."));
        assert!(!super::is_terminated("yt-dlp listing failed for UC1: HTTP Error 404: Not Found"));
        assert!(!super::is_terminated("yt-dlp timed out after 120s"));
    }

    use super::*;
    use crate::models::VideoStatus;
    use std::path::{Path, PathBuf};

    /// A machine where nothing resolved but yt-dlp itself.
    fn bare() -> Runner {
        Runner { program: PathBuf::from("yt-dlp"), ffmpeg_location: None, deno: None,
                 cookies: Cookies::None }
    }

    /// One where everything did, with Firefox cookies -- the Linux setup this
    /// app grew up on.
    fn full() -> Runner {
        Runner {
            program: PathBuf::from("/bin/dir/yt-dlp"),
            ffmpeg_location: Some(PathBuf::from("/tools/ffmpeg dir")),
            deno: Some(PathBuf::from("/tools/deno")),
            cookies: Cookies::Browser("firefox".into()),
        }
    }

    fn args() -> Vec<String> {
        download_args(&full(), &Quality::default(), "abc123",
                      &PathBuf::from("/out/Some Title [abc123].mkv"), &PathBuf::from("/tmp/p.txt"))
    }

    fn value_of(a: &[String], flag: &str) -> Option<String> {
        a.iter().position(|x| x == flag).map(|i| a[i + 1].clone())
    }

    /// Every argv yt-dlp is ever given for a video, for a runner.
    fn video_argvs(r: &Runner) -> [Vec<String>; 3] {
        [
            download_args(r, &Quality::default(), "abc123", &PathBuf::from("/out/x.mkv"),
                          &PathBuf::from("/tmp/p")),
            probe_args(r, &Quality::default(), "https://www.youtube.com/watch?v=x", "/out",
                       "%(title)s.%(ext)s"),
            formats_args(r, &Quality::default(), "x"),
        ]
    }

    #[test]
    fn every_required_flag_is_present() {
        for r in [bare(), full()] {
            let a = download_args(&r, &Quality::default(), "abc123",
                                  &PathBuf::from("/out/x.mkv"), &PathBuf::from("/tmp/p"));
            for flag in ["-f", "bv*+ba/b", "--merge-output-format", "mkv",
                         "--embed-thumbnail", "--convert-thumbnails", "jpg",
                         "--no-playlist", "--color", "no_color", "--newline"] {
                assert!(a.iter().any(|x| x == flag), "missing {flag} in {a:?}");
            }
        }
    }

    #[test]
    fn the_default_quality_gives_todays_download_argv() {
        // Pinned from the argv before quality existed.
        let a = download_args(&bare(), &Quality::default(), "abc123",
                              &PathBuf::from("/out/x.mkv"), &PathBuf::from("/tmp/p"));
        let mut want: Vec<String> = ["-f", "bv*+ba/b", "--merge-output-format", "mkv",
                                     "--no-playlist", "--color", "no_color",
                                     "--remote-components", "ejs:npm",
                                     "--remote-components", "ejs:github"]
            .iter().map(|x| x.to_string()).collect();
        want.splice(7..7, encoding_args());
        if !cfg!(windows) {
            want.push("--no-windows-filenames".into());
        }
        want.extend(["--embed-thumbnail", "--convert-thumbnails", "jpg", "--newline",
                     "--progress-template", PROGRESS_TEMPLATE,
                     "--print-to-file", "after_move:filepath", "/tmp/p",
                     "-o", "/out/x.mkv", "https://www.youtube.com/watch?v=abc123"]
                    .iter().map(|x| x.to_string()));
        assert_eq!(a, want);
    }

    #[test]
    fn a_thumbnail_is_embedded_only_into_a_container_that_takes_one() {
        let has = |q: &Quality| download_args(&bare(), q, "x", &PathBuf::from("/o/x"),
                                              &PathBuf::from("/p"))
            .contains(&"--embed-thumbnail".to_string());
        let audio = |f: &str| Quality { mode: "audio".into(), audio_format: f.into(),
                                        ..Quality::default() };
        assert!(has(&Quality::default()));
        assert!(has(&Quality { container: "mp4".into(), ..Quality::default() }));
        assert!(has(&audio("mp3")) && has(&audio("m4a")) && has(&audio("opus")));
        assert!(!has(&audio("original")), "original audio may be webm");
        assert!(!has(&Quality { format: "251".into(), ..Quality::default() }),
                "one raw stream may be webm");
        assert!(has(&Quality { format: "399+251".into(), ..Quality::default() }));
    }

    #[test]
    fn probe_and_download_share_the_quality_flags() {
        let q = Quality { max_height: 720, vcodec: "h264".into(), container: "mp4".into(),
                          ..Quality::default() };
        let p = probe_args(&full(), &q, "u", "/out", "t");
        let d = download_args(&full(), &q, "x", &PathBuf::from("/o/x.mp4"), &PathBuf::from("/p"));
        let f = formats_args(&full(), &q, "x");
        for a in [&p, &d, &f] {
            assert_eq!(value_of(a, "-S").as_deref(), Some("res:720,vcodec:h264"), "{a:?}");
            assert_eq!(value_of(a, "--merge-output-format").as_deref(), Some("mp4"), "{a:?}");
        }
    }

    #[test]
    fn the_format_listing_is_one_json_dump_of_the_watch_page() {
        let a = formats_args(&full(), &Quality::default(), "abc");
        assert!(a.contains(&"-J".to_string()));
        assert!(a.contains(&"--no-playlist".to_string()));
        assert!(a.contains(&"--no-warnings".to_string()));
        assert!(!a.contains(&"--embed-thumbnail".to_string()));
        assert_eq!(a.last().unwrap(), "https://www.youtube.com/watch?v=abc");
    }

    /// A real `-J` of dQw4w9WgXcQ (2026-09-29), trimmed to a representative
    /// subset: storyboard, muxed 18, audio 249/251/774/140, and video in all
    /// three codecs from 360p to 2160p.
    const FORMATS_FIXTURE: &str = include_str!("../tests/fixtures/dQw4w9WgXcQ_formats.json");

    #[test]
    fn a_real_format_dump_parses_into_tracks_and_defaults() {
        let f = parse_formats(FORMATS_FIXTURE).unwrap();
        assert_eq!(f.title, "Rick Astley - Never Gonna Give You Up (Official Video) (4K Remaster)");

        let ids: Vec<&str> = f.video.iter().map(|t| t.id.as_str()).collect();
        // Height descending, then size descending; no storyboard, no muxed 18.
        assert_eq!(ids, vec!["313", "401", "137", "248", "399", "136", "134", "243", "396"]);
        let v137 = f.video.iter().find(|t| t.id == "137").unwrap();
        assert_eq!(v137.height, 1080);
        assert_eq!(v137.vcodec, "h264");
        assert_eq!(v137.codec, "avc1.640028");
        assert_eq!(v137.fps, Some(25.0));
        assert_eq!(v137.ext, "mp4");
        assert_eq!(v137.size, Some(80911999));
        assert!(!v137.hdr);
        assert_eq!(f.video.iter().find(|t| t.id == "401").unwrap().vcodec, "av1");
        assert_eq!(f.video.iter().find(|t| t.id == "313").unwrap().vcodec, "vp9");

        let aids: Vec<&str> = f.audio.iter().map(|t| t.id.as_str()).collect();
        assert_eq!(aids, vec!["774", "140", "251", "249"], "bitrate descending");
        let a774 = &f.audio[0];
        assert_eq!(a774.acodec, "opus");
        assert_eq!(a774.language.as_deref(), Some("en"));
        assert_eq!(a774.ext, "webm");
        assert!(a774.abr.unwrap() > 257.0);
        assert_eq!(f.audio[1].acodec, "aac");

        assert_eq!(f.default_video.as_deref(), Some("401"));
        assert_eq!(f.default_audio.as_deref(), Some("774"));
    }

    #[test]
    fn the_payload_is_camel_case() {
        let f = parse_formats(FORMATS_FIXTURE).unwrap();
        let j = serde_json::to_value(&f).unwrap();
        assert_eq!(j["defaultVideo"], "401");
        assert_eq!(j["defaultAudio"], "774");
        assert!(j["video"][0].get("hdr").is_some());
        assert!(j["audio"][0].get("abr").is_some());
    }

    /// `probe_formats` end to end against YouTube:
    ///
    ///     cargo test --manifest-path src-tauri/Cargo.toml real_format_listing -- --ignored --nocapture
    #[tokio::test]
    #[ignore = "requires network, yt-dlp and deno"]
    async fn real_format_listing() {
        let settings = crate::config::load().unwrap_or_default();
        let tools = crate::tools::Tools::new(crate::tools::bin_dir(), reqwest::Client::new());
        let inv = tools.ytdlp(&settings).await.expect("a yt-dlp");
        for q in [Quality::default(),
                  Quality { max_height: 720, vcodec: "h264".into(), ..Quality::default() }] {
            let f = formats(&inv, "dQw4w9WgXcQ", &q).await.expect("a format list");
            println!("{q:?}: {} video, {} audio, default {:?}+{:?}", f.video.len(), f.audio.len(),
                     f.default_video, f.default_audio);
            assert!(!f.video.is_empty() && !f.audio.is_empty());
            let dv = f.default_video.as_deref().expect("a default video pick");
            assert!(f.default_audio.is_some());
            if q.max_height == 720 {
                let t = f.video.iter().find(|t| t.id == dv).unwrap();
                assert_eq!((t.height, t.vcodec), (720, "h264"), "{t:?}");
            }
        }
    }

    #[test]
    fn hls_is_kept_only_when_it_is_all_there_is() {
        let both = r#"{"title":"T","formats":[
            {"format_id":"96","ext":"mp4","vcodec":"avc1","acodec":"mp4a","height":1080,"protocol":"m3u8_native"},
            {"format_id":"301","ext":"mp4","vcodec":"avc1.4d","acodec":"none","height":1080,"protocol":"m3u8_native"},
            {"format_id":"137","ext":"mp4","vcodec":"avc1.64","acodec":"none","height":1080,"protocol":"https"},
            {"format_id":"140","ext":"m4a","vcodec":"none","acodec":"mp4a.40.2","abr":129,"protocol":"https"}
        ],"format_id":"137+140"}"#;
        let f = parse_formats(both).unwrap();
        assert_eq!(f.video.iter().map(|t| t.id.as_str()).collect::<Vec<_>>(), vec!["137"]);

        let only_hls = r#"{"title":"T","formats":[
            {"format_id":"301","ext":"mp4","vcodec":"avc1.4d","acodec":"none","height":720,"protocol":"m3u8_native"},
            {"format_id":"234","ext":"mp4","vcodec":"none","acodec":"mp4a.40.2","protocol":"m3u8_native"}
        ]}"#;
        let f = parse_formats(only_hls).unwrap();
        assert_eq!(f.video.len(), 1);
        assert_eq!(f.audio.len(), 1);
        assert_eq!(f.default_video, None);
    }

    #[test]
    fn hdr_and_a_single_audio_pick_are_recognised() {
        let j = r#"{"title":"T","format_id":"251","vcodec":"none","formats":[
            {"format_id":"337","ext":"webm","vcodec":"vp09.02.51.10","acodec":"none","height":2160,"fps":60,"dynamic_range":"HDR10"},
            {"format_id":"251","ext":"webm","vcodec":"none","acodec":"opus","abr":130}
        ]}"#;
        let f = parse_formats(j).unwrap();
        assert!(f.video[0].hdr);
        assert_eq!(f.video[0].vcodec, "vp9");
        assert_eq!(f.default_audio.as_deref(), Some("251"));
        assert_eq!(f.default_video, None);
        assert!(parse_formats("not json").is_err());
    }

    #[test]
    fn no_cookie_source_passes_no_cookie_flag_at_all() {
        // `cookies_browser: "auto"` on a machine with no Firefox resolves to
        // this, and a `--cookies-from-browser firefox` there fails every run.
        for a in video_argvs(&bare()) {
            assert!(!a.iter().any(|x| x.starts_with("--cookies")), "{a:?}");
        }
    }

    #[test]
    fn a_browser_spec_is_passed_verbatim() {
        let r = Runner { cookies: Cookies::Browser("chrome:Profile 1".into()), ..bare() };
        for a in video_argvs(&r) {
            assert_eq!(value_of(&a, "--cookies-from-browser").as_deref(),
                       Some("chrome:Profile 1"));
            assert!(!a.contains(&"--cookies".to_string()));
        }
    }

    #[test]
    fn a_cookies_file_is_passed_as_cookies_and_not_as_a_browser() {
        let r = Runner { cookies: Cookies::File(PathBuf::from("/home/u/my cookies.txt")),
                         ..bare() };
        for a in video_argvs(&r) {
            assert_eq!(value_of(&a, "--cookies").as_deref(), Some("/home/u/my cookies.txt"));
            assert!(!a.contains(&"--cookies-from-browser".to_string()));
        }
    }

    #[test]
    fn resolved_ffmpeg_and_deno_are_handed_to_every_run() {
        let r = full();
        let listing = flat_playlist_args(&r, "UC1", 30);
        for a in video_argvs(&r).iter().chain([&listing]) {
            assert_eq!(value_of(a, "--ffmpeg-location").as_deref(), Some("/tools/ffmpeg dir"));
            assert_eq!(value_of(a, "--js-runtimes").as_deref(), Some("deno:/tools/deno"));
        }
    }

    #[test]
    fn unresolved_ffmpeg_and_deno_leave_yt_dlp_to_search_for_itself() {
        let r = bare();
        let listing = flat_playlist_args(&r, "UC1", 30);
        for a in video_argvs(&r).iter().chain([&listing]) {
            assert!(!a.contains(&"--ffmpeg-location".to_string()), "{a:?}");
            assert!(!a.contains(&"--js-runtimes".to_string()), "{a:?}");
        }
    }

    #[test]
    fn every_run_asks_for_utf8_output_on_windows_and_only_there() {
        // A Windows pipe otherwise gets the ANSI code page, and yt-dlp drops
        // every character outside it from titles and paths.
        for r in [bare(), full()] {
            let listing = flat_playlist_args(&r, "UC1", 30);
            for a in video_argvs(&r).iter().chain([&listing]) {
                assert_eq!(value_of(a, "--encoding").as_deref(),
                           cfg!(windows).then_some("utf-8"), "{a:?}");
            }
        }
    }

    #[test]
    fn windows_filenames_are_kept_minimal_only_off_windows() {
        for a in video_argvs(&full()) {
            assert_eq!(a.contains(&"--no-windows-filenames".to_string()), !cfg!(windows),
                       "{a:?}");
        }
    }

    #[test]
    fn a_listing_never_reads_cookies() {
        let r = Runner { cookies: Cookies::File(PathBuf::from("/c.txt")), ..full() };
        let a = flat_playlist_args(&r, "UC1", 30);
        assert!(!a.iter().any(|x| x.starts_with("--cookies")), "{a:?}");
        let r = Runner { cookies: Cookies::Browser("firefox".into()), ..full() };
        let a = flat_playlist_args(&r, "UC1", 30);
        assert!(!a.iter().any(|x| x.starts_with("--cookies")), "{a:?}");
    }

    #[test]
    fn both_remote_components_are_passed() {
        let a = args();
        let vals: Vec<&String> = a.iter().zip(a.iter().skip(1))
            .filter(|(f, _)| *f == "--remote-components").map(|(_, v)| v).collect();
        assert_eq!(vals, vec!["ejs:npm", "ejs:github"]);
    }

    #[test]
    fn the_output_path_is_forced_absolutely_so_yt_dlp_cannot_pick_a_name() {
        let a = args();
        let at = |flag: &str| a.iter().position(|x| x == flag).map(|i| a[i + 1].clone());
        assert_eq!(at("-o").as_deref(), Some("/out/Some Title [abc123].mkv"));
        assert!(!a.iter().any(|x| x == "-P"), "an absolute -o replaces -P entirely");
        assert_eq!(a.last().unwrap(), "https://www.youtube.com/watch?v=abc123");
    }

    #[test]
    fn percent_signs_in_a_path_are_escaped_for_the_output_template() {
        let a = download_args(&bare(), &Quality::default(), "x",
                              &PathBuf::from("/out/100% real (2).mkv"), &PathBuf::from("/tmp/p.txt"));
        let i = a.iter().position(|x| x == "-o").unwrap();
        assert_eq!(a[i + 1], "/out/100%% real (2).mkv");
        assert_eq!(escape_out_template(&PathBuf::from("/a/b.mkv")), "/a/b.mkv");
    }

    #[test]
    fn the_probe_resolves_the_template_and_asks_for_every_field() {
        let a = probe_args(&full(), &Quality::default(), "https://www.youtube.com/watch?v=x",
                           "/out", "%(title)s.%(ext)s");
        assert!(a.contains(&"--simulate".to_string()));
        let at = |flag: &str| a.iter().position(|x| x == flag).map(|i| a[i + 1].clone());
        assert_eq!(at("-P").as_deref(), Some("/out"));
        assert_eq!(at("-o").as_deref(), Some("%(title)s.%(ext)s"));
        // Order matters: parse_probe_output reads these lines positionally.
        let prints: Vec<&String> = a.iter().zip(a.iter().skip(1))
            .filter(|(f, _)| *f == "--print").map(|(_, v)| v).collect();
        assert_eq!(prints, vec!["%(id)s", "%(title)s", "%(duration)s", "%(timestamp)s",
                                "%(channel)s", "%(channel_id)s", "filename"]);
        assert!(a.contains(&"mkv".to_string()), "ext must resolve to mkv in the probe");
    }

    #[test]
    fn probe_output_parses_positionally() {
        let out = "J1WoNuemKOg\n                   We sent a balloon | to space\n                   1365\n                   1786977016\n                   Veritasium\n                   UCHnyfMqiRRG1u-2MsSQLbXA\n                   /home/u/Videos/Veritasium/We sent a balloon [J1WoNuemKOg].mkv\n";
        let p = parse_probe_output(out).unwrap();
        assert_eq!(p.id, "J1WoNuemKOg");
        assert_eq!(p.title, "We sent a balloon | to space", "pipes in titles are safe");
        assert_eq!(p.duration_secs, Some(1365));
        assert_eq!(p.published_at, Some(1786977016));
        assert_eq!(p.channel_title, "Veritasium");
        assert_eq!(p.channel_id, "UCHnyfMqiRRG1u-2MsSQLbXA");
        assert!(p.intended_path.ends_with(".mkv"));
    }

    #[test]
    fn probe_output_handles_na_fields_and_rejects_short_output() {
        let out = "abc\nT\nNA\nNA\nCh\nUC1\n/tmp/x.mkv\n";
        let p = parse_probe_output(out).unwrap();
        assert_eq!(p.duration_secs, None);
        assert_eq!(p.published_at, None);
        assert!(parse_probe_output("abc\nT\n").is_err(), "truncated output must error");
    }

    #[test]
    fn unique_path_returns_the_intended_path_when_it_is_free() {
        let p = PathBuf::from("/out/Video [id].mkv");
        assert_eq!(unique_path(&p, |_| false), p);
    }

    #[test]
    fn unique_path_appends_a_counter_before_the_extension() {
        let p = PathBuf::from("/out/Video [id].mkv");
        let taken = |c: &Path| c == Path::new("/out/Video [id].mkv");
        assert_eq!(unique_path(&p, taken), PathBuf::from("/out/Video [id] (2).mkv"));

        let taken2 = |c: &Path| {
            c == Path::new("/out/Video [id].mkv") || c == Path::new("/out/Video [id] (2).mkv")
        };
        assert_eq!(unique_path(&p, taken2), PathBuf::from("/out/Video [id] (3).mkv"));
    }

    #[test]
    fn unique_path_preserves_dots_in_the_stem_and_handles_no_extension() {
        let p = PathBuf::from("/out/S01.E02. Pilot.mkv");
        assert_eq!(unique_path(&p, |c| c == Path::new("/out/S01.E02. Pilot.mkv")),
                   PathBuf::from("/out/S01.E02. Pilot (2).mkv"));
        let n = PathBuf::from("/out/noext");
        assert_eq!(unique_path(&n, |c| c == Path::new("/out/noext")),
                   PathBuf::from("/out/noext (2)"));
    }

    #[test]
    fn progress_template_and_print_file_are_wired() {
        let a = args();
        assert!(a.iter().any(|x| x.starts_with("MYTUBE|")));
        let i = a.iter().position(|x| x == "--print-to-file").unwrap();
        assert_eq!(a[i + 1], "after_move:filepath");
        assert_eq!(a[i + 2], "/tmp/p.txt");
    }

    #[test]
    fn flat_playlist_uses_json_lines_not_pipe_separated_print() {
        let a = flat_playlist_args(&bare(), "UC1", 30);
        assert!(a.contains(&"--flat-playlist".to_string()));
        assert!(a.contains(&"-j".to_string()), "must use JSON lines");
        assert!(!a.contains(&"--print".to_string()), "--print splits on | inside titles");
        let i = a.iter().position(|x| x == "--playlist-end").unwrap();
        assert_eq!(a[i + 1], "30");
        assert_eq!(a.last().unwrap(), "https://www.youtube.com/channel/UC1/videos");
    }

    #[test]
    fn progress_lines_parse() {
        let (p, s, e) = parse_progress_line("MYTUBE|  45.2%|  1.20MiB/s|00:12").unwrap();
        assert!((p - 45.2).abs() < 0.01);
        assert_eq!(s, "1.20MiB/s");
        assert_eq!(e, "00:12");
        assert_eq!(parse_progress_line("MYTUBE|100%|10MiB/s|00:00").unwrap().0, 100.0);
    }

    #[test]
    fn malformed_or_unrelated_progress_lines_are_skipped_not_fatal() {
        for bad in ["", "[download] Destination: x.mkv", "MYTUBE|", "MYTUBE|NA|NA|NA",
                    "MYTUBE|abc|x|y", "MYTUBE|50%|only-two"] {
            assert!(parse_progress_line(bad).is_none(), "{bad:?} should yield None");
        }
    }

    #[test]
    fn flat_entries_parse_including_titles_containing_pipes() {
        let line = r#"{"id":"abc","title":"A | B | C","duration":1365,"view_count":1800000}"#;
        let e = parse_flat_entry(line).unwrap();
        assert_eq!(e.id, "abc");
        assert_eq!(e.title, "A | B | C");
        assert_eq!(e.duration_secs, Some(1365));
        assert_eq!(e.view_count, Some(1800000));
        assert_eq!(e.live_status, None);
    }

    #[test]
    fn flat_entries_tolerate_nulls_and_bad_json() {
        let e = parse_flat_entry(r#"{"id":"x","title":"T","duration":null,"live_status":"is_upcoming"}"#).unwrap();
        assert_eq!(e.duration_secs, None);
        assert_eq!(e.live_status.as_deref(), Some("is_upcoming"));
        // Fractional durations round down to whole seconds.
        assert_eq!(parse_flat_entry(r#"{"id":"x","title":"T","duration":60.7}"#).unwrap().duration_secs, Some(60));
        assert!(parse_flat_entry("not json").is_none());
        assert!(parse_flat_entry(r#"{"title":"no id"}"#).is_none());
    }

    #[test]
    fn flat_listing_requests_approximate_dates() {
        let a = flat_playlist_args(&bare(), "UC1", 30);
        let i = a.iter().position(|x| x == "--extractor-args")
            .expect("flat listings must ask for approximate dates");
        assert_eq!(a[i + 1], "youtubetab:approximate_date");
    }

    #[test]
    fn flat_entries_carry_an_upload_date() {
        let e = parse_flat_entry(
            r#"{"id":"x","title":"T","duration":60,"timestamp":1787011200,"upload_date":"20260818"}"#
        ).unwrap();
        assert_eq!(e.published_at, Some(1787011200), "timestamp wins when present");

        let only_date = parse_flat_entry(r#"{"id":"x","title":"T","upload_date":"20260818"}"#).unwrap();
        assert_eq!(only_date.published_at, Some(1787011200), "falls back to upload_date");

        let neither = parse_flat_entry(r#"{"id":"x","title":"T"}"#).unwrap();
        assert_eq!(neither.published_at, None);

        let junk = parse_flat_entry(r#"{"id":"x","title":"T","upload_date":"nonsense"}"#).unwrap();
        assert_eq!(junk.published_at, None, "malformed dates must not panic");
    }

    /// The field that tells a members-only upload from a public one. It is the
    /// only way to know: the listing serves both to anyone, membership or not.
    #[test]
    fn flat_entries_carry_their_availability() {
        let members = parse_flat_entry(
            r#"{"id":"x","title":"T","duration":7982,"availability":"subscriber_only"}"#
        ).unwrap();
        assert_eq!(members.availability.as_deref(), Some("subscriber_only"));

        let public = parse_flat_entry(r#"{"id":"y","title":"T","duration":60}"#).unwrap();
        assert_eq!(public.availability, None, "a public entry says nothing at all");
    }

    #[test]
    fn status_is_derived_from_live_status_and_duration() {
        assert_eq!(status_from(None, Some(600)), VideoStatus::Ready);
        assert_eq!(status_from(Some("not_live"), Some(600)), VideoStatus::Ready);
        assert_eq!(status_from(Some("was_live"), Some(600)), VideoStatus::Ready);
        assert_eq!(status_from(Some("post_live"), Some(600)), VideoStatus::Ready);
        assert_eq!(status_from(Some("is_live"), None), VideoStatus::Live);
        assert_eq!(status_from(Some("is_upcoming"), None), VideoStatus::Upcoming);
        // No duration yet and not explicitly live: retry next poll.
        assert_eq!(status_from(None, None), VideoStatus::Pending);
        assert_eq!(status_from(None, Some(0)), VideoStatus::Pending);
    }

    #[test]
    fn thumbnail_url_is_deterministic() {
        assert_eq!(thumb_url_for("abc"), "https://i.ytimg.com/vi/abc/hqdefault.jpg");
    }

    /// The tests below run real processes, found through `sh`, `sleep` and
    /// `pgrep`; the tree kill itself has a Windows test in `proc`.
    #[cfg(unix)]
    mod processes {
        use super::*;
        use std::sync::Arc;

        /// A unique `sleep` duration, so `pgrep` can find this test's own child and
        /// nobody else's.
        fn probe_seconds() -> String {
            format!("300.{}", std::process::id())
        }

        fn child_alive(pattern: &str) -> bool {
            let out = std::process::Command::new("pgrep")
                .arg("-f").arg(pattern)
                .output()
                .expect("pgrep is available");
            !out.stdout.is_empty()
        }

        async fn gone_soon(pattern: &str) -> bool {
            for _ in 0..40 {
                if !child_alive(pattern) {
                    return true;
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
            // Leave nothing behind for the next run if the assertion is about to fail.
            let _ = std::process::Command::new("pkill").arg("-f").arg(pattern).status();
            false
        }

        #[tokio::test]
        async fn a_child_that_finishes_in_time_returns_its_output() {
            let out = run_with_timeout(Path::new("echo"), vec![s("hello")],
                                       Duration::from_secs(10))
                .await
                .expect("echo should not time out");
            assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "hello");
        }

        #[tokio::test]
        async fn a_child_that_overruns_its_budget_is_an_error_rather_than_a_hang() {
            let err = run_with_timeout(Path::new("sleep"), vec![probe_seconds()],
                                       Duration::from_millis(100))
                .await
                .expect_err("a 300s sleep must not survive a 100ms budget");
            assert!(err.to_string().contains("timed out"), "{err}");
        }

        #[tokio::test]
        async fn a_timed_out_child_is_killed_rather_than_left_running() {
            // The timeout is only real if the process actually goes away: dropping
            // the future otherwise leaves yt-dlp running, and the app would collect
            // one stuck child per poll interval forever.
            let secs = probe_seconds();
            let pattern = format!("sleep {secs}");
            let _ = run_with_timeout(Path::new("sleep"), vec![secs.clone()],
                                     Duration::from_millis(100)).await;
            assert!(gone_soon(&pattern).await,
                    "`{pattern}` outlived the timeout that was supposed to kill it");
        }

        #[tokio::test]
        async fn a_timed_out_listing_kills_the_grandchild_too() {
            // The standalone yt-dlp is a PyInstaller bootloader that runs the real
            // yt-dlp as its child. Killing only the process we spawned would leave
            // that child wedged -- the very thing the timeout exists to prevent.
            let secs = format!("305.{}", std::process::id());
            let _ = run_with_timeout(Path::new("sh"),
                                     vec![s("-c"), format!("sleep {secs} & sleep {secs}")],
                                     Duration::from_millis(300)).await;
            assert!(gone_soon(&secs).await, "a grandchild outlived the listing's timeout");
        }

        /// A stand-in yt-dlp: a script that ignores its arguments and runs `body`.
        ///
        /// Written by a `sh` of its own rather than `std::fs::write`. The tests
        /// run in parallel threads, and a fork in any of them while this process
        /// holds the file open for writing hands the child a copy of that fd --
        /// after which exec'ing the script fails with ETXTBSY ("Text file busy")
        /// until the child gets round to its own exec. Measured: 2 runs in 12.
        fn fake_ytdlp(name: &str, body: &str) -> PathBuf {
            let dir = std::env::temp_dir()
                .join(format!("mytube-fake-ytdlp-{}-{name}", std::process::id()));
            std::fs::create_dir_all(&dir).unwrap();
            let path = dir.join("yt-dlp");
            let ok = std::process::Command::new("sh")
                .arg("-c")
                .arg(r#"printf '%s\n' "$1" > "$2" && chmod 755 "$2""#)
                .arg("sh")
                .arg(format!("#!/bin/sh\n{body}"))
                .arg(&path)
                .status()
                .expect("sh is available");
            assert!(ok.success(), "could not write {}", path.display());
            path
        }

        async fn invocation(program: PathBuf) -> Invocation {
            Invocation {
                runner: Runner { program, ..bare() },
                _lease: Arc::new(tokio::sync::RwLock::new(())).read_owned().await,
            }
        }

        #[tokio::test]
        async fn the_probe_runs_the_yt_dlp_it_was_handed() {
            let program = fake_ytdlp("probe-ok",
                "printf 'abc\\nT\\n60\\nNA\\nCh\\nUC1\\n/out/T [abc].mkv\\n'");
            let inv = invocation(program.clone()).await;
            let p = probe(&inv, &Quality::default(), "https://www.youtube.com/watch?v=abc", "/out",
                          "%(title)s")
                .await;
            let _ = std::fs::remove_dir_all(program.parent().unwrap());
            let p = p.expect("the fake yt-dlp prints a full probe");
            assert_eq!(p.id, "abc");
            assert_eq!(p.duration_secs, Some(60));
            assert_eq!(p.intended_path, "/out/T [abc].mkv");
        }

        #[tokio::test]
        async fn a_listing_runs_the_yt_dlp_it_was_handed() {
            let program = fake_ytdlp("listing-ok",
                r#"echo '{"id":"a","title":"A","duration":60}'; echo '{"id":"b","title":"B"}'"#);
            let inv = invocation(program.clone()).await;
            let entries = flat_playlist(&inv, "UC1", 30).await;
            let _ = std::fs::remove_dir_all(program.parent().unwrap());
            let ids: Vec<String> = entries.expect("the fake listing succeeds")
                .into_iter().map(|e| e.id).collect();
            assert_eq!(ids, vec!["a", "b"]);
        }

        #[tokio::test]
        async fn an_aborted_probe_kills_the_whole_tree() {
            // Cancelling a download in phase 1 aborts its task, which drops the
            // probe mid-flight. Everything the probe started has to go with it.
            let secs = format!("306.{}", std::process::id());
            let program = fake_ytdlp("probe-abort", &format!("sleep {secs} & sleep {secs}"));
            let inv = invocation(program.clone()).await;
            let task = tokio::spawn(async move {
                let _ = probe(&inv, &Quality::default(), "u", "/out", "t").await;
            });
            for _ in 0..80 {
                if child_alive(&secs) {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(25)).await;
            }
            let started = child_alive(&secs);

            task.abort();
            let _ = task.await;

            let gone = gone_soon(&secs).await;
            let _ = std::fs::remove_dir_all(program.parent().unwrap());
            assert!(started, "the fake probe should be running before the abort");
            assert!(gone, "something the probe started outlived its abort");
        }
    }
}

