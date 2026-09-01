use anyhow::{anyhow, Result};
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::models::{FlatEntry, ProbeInfo, VideoStatus};

pub const PROGRESS_PREFIX: &str = "MYTUBE|";
const PROGRESS_TEMPLATE: &str =
    "MYTUBE|%(progress._percent_str)s|%(progress._speed_str)s|%(progress._eta_str)s";

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

/// Flags shared by the probe and the real download, so the probe resolves the
/// same extension the download will actually produce.
fn common_format_args() -> Vec<String> {
    vec![
        s("-f"), s("bv*+ba/b"),
        s("--merge-output-format"), s("mkv"),
        s("--no-playlist"),
        s("--color"), s("no_color"),
        s("--cookies-from-browser"), s("firefox"),
        s("--remote-components"), s("ejs:npm"),
        s("--remote-components"), s("ejs:github"),
        s("--no-windows-filenames"),
    ]
}

/// Phase 1. Resolves the output template to a concrete path and returns metadata.
/// The `--print` order is load-bearing: `parse_probe_output` reads lines positionally.
pub fn probe_args(url: &str, download_dir: &str, filename_template: &str) -> Vec<String> {
    let mut a = common_format_args();
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
pub fn download_args(video_id: &str, out_path: &Path, print_file: &Path) -> Vec<String> {
    let mut a = common_format_args();
    a.extend([
        s("--embed-thumbnail"),
        s("--convert-thumbnails"), s("jpg"),
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
pub async fn probe(url: &str, download_dir: &str, filename_template: &str) -> Result<ProbeInfo> {
    let out = tokio::process::Command::new("yt-dlp")
        .args(probe_args(url, download_dir, filename_template))
        .output().await
        .map_err(|e| anyhow!("could not run yt-dlp: {e}"))?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        return Err(anyhow!("{}", err.lines().rev().find(|l| l.contains("ERROR"))
            .unwrap_or("yt-dlp could not read that video")));
    }
    parse_probe_output(&String::from_utf8_lossy(&out.stdout))
}

/// JSON lines, never `--print` with a `|` separator: titles contain `|`.
pub fn flat_playlist_args(channel_id: &str, limit: u32) -> Vec<String> {
    vec![
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
    ]
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
/// `kill_on_drop` is what makes the ceiling real: dropping the future on
/// timeout otherwise leaves the child running with nothing left to reap it.
async fn run_with_timeout(
    program: &str,
    args: Vec<String>,
    limit: Duration,
) -> Result<std::process::Output> {
    let run = tokio::process::Command::new(program)
        .args(args)
        .kill_on_drop(true)
        .output();
    match tokio::time::timeout(limit, run).await {
        Ok(finished) => finished.map_err(|e| anyhow!("could not run {program}: {e}")),
        Err(_) => Err(anyhow!("{program} timed out after {}s", limit.as_secs())),
    }
}

pub async fn flat_playlist(channel_id: &str, limit: u32) -> Result<Vec<FlatEntry>> {
    let out = run_with_timeout("yt-dlp", flat_playlist_args(channel_id, limit), LISTING_TIMEOUT)
        .await?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        return Err(anyhow!("yt-dlp listing failed for {channel_id}: {}",
                           err.lines().last().unwrap_or("unknown error")));
    }
    Ok(String::from_utf8_lossy(&out.stdout)
        .lines().filter_map(parse_flat_entry).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::VideoStatus;
    use std::path::{Path, PathBuf};

    fn args() -> Vec<String> {
        download_args("abc123", &PathBuf::from("/out/Some Title [abc123].mkv"),
                      &PathBuf::from("/tmp/p.txt"))
    }

    #[test]
    fn every_required_flag_is_present() {
        let a = args();
        for flag in ["-f", "bv*+ba/b", "--merge-output-format", "mkv",
                     "--embed-thumbnail", "--convert-thumbnails", "jpg",
                     "--cookies-from-browser", "firefox",
                     "--no-windows-filenames", "--no-playlist",
                     "--color", "no_color", "--newline"] {
            assert!(a.iter().any(|x| x == flag), "missing {flag} in {a:?}");
        }
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
        let a = download_args("x", &PathBuf::from("/out/100% real (2).mkv"),
                              &PathBuf::from("/tmp/p.txt"));
        let i = a.iter().position(|x| x == "-o").unwrap();
        assert_eq!(a[i + 1], "/out/100%% real (2).mkv");
        assert_eq!(escape_out_template(&PathBuf::from("/a/b.mkv")), "/a/b.mkv");
    }

    #[test]
    fn the_probe_resolves_the_template_and_asks_for_every_field() {
        let a = probe_args("https://www.youtube.com/watch?v=x", "/out", "%(title)s.%(ext)s");
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
        let a = flat_playlist_args("UC1", 30);
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
        let a = flat_playlist_args("UC1", 30);
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

    #[tokio::test]
    async fn a_child_that_finishes_in_time_returns_its_output() {
        let out = run_with_timeout("echo", vec![s("hello")], Duration::from_secs(10))
            .await
            .expect("echo should not time out");
        assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "hello");
    }

    #[tokio::test]
    async fn a_child_that_overruns_its_budget_is_an_error_rather_than_a_hang() {
        let err = run_with_timeout("sleep", vec![probe_seconds()], Duration::from_millis(100))
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
        let _ = run_with_timeout("sleep", vec![secs.clone()], Duration::from_millis(100)).await;

        for _ in 0..40 {
            if !child_alive(&pattern) {
                return;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        panic!("`{pattern}` outlived the timeout that was supposed to kill it");
    }
}
