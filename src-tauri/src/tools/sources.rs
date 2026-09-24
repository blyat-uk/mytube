//! Where each managed tool comes from, per target, and how to read what the
//! publishers put beside their downloads: checksum files, release redirects and
//! `--version` output. Everything here is pure text in, text out; the network
//! lives in `install.rs`.
//!
//! What the sources actually serve, observed with curl on 2026-09-24:
//!
//! - `https://github.com/<owner>/<repo>/releases/latest` answers `302` with
//!   `location: https://github.com/<owner>/<repo>/releases/tag/<tag>`. Seen:
//!   yt-dlp/yt-dlp -> `2026.08.19`, yt-dlp/yt-dlp-nightly-builds ->
//!   `2026.09.16.232951`, denoland/deno -> `v2.9.7`, yt-dlp/FFmpeg-Builds ->
//!   `latest` (a moving tag, rebuilt daily). A yt-dlp tag is exactly what that
//!   build's `--version` prints, on both channels.
//! - Release assets answer `302` to `release-assets.githubusercontent.com`,
//!   then `200`. Sizes: yt-dlp_linux 40 MB, yt-dlp_macos 37 MB, yt-dlp.exe
//!   18 MB; deno zips ~40 MB; ffmpeg-master-latest-linux64-gpl.tar.xz 153 MB,
//!   -win64-gpl.zip 196 MB.
//! - yt-dlp's `SHA2-256SUMS` (both channels): one `<64 lowercase hex>  <name>`
//!   per line, two spaces, LF, covering `yt-dlp`, `yt-dlp.exe`,
//!   `yt-dlp_linux`, `yt-dlp_linux_aarch64`, `yt-dlp_macos`, the musllinux,
//!   x86 and arm64 exes and the zips. Names are prefixes of one another
//!   (`yt-dlp`, `yt-dlp_linux`, `yt-dlp_linux.zip`), so a line matches on the
//!   exact name field, never a substring.
//! - deno's `<asset>.sha256sum` comes in **two shapes**. Unix targets:
//!   `<64 lowercase hex>  deno-<triple>.zip`. Windows (`x86_64-pc-windows-msvc`)
//!   is PowerShell `Get-FileHash` output, CRLF, with the hash in UPPERCASE:
//!   `Algorithm : SHA256` / `Hash      : A0C3…2238` / `Path      : C:\a\…zip`.
//!   Both hashes matched a local `sha256sum` of the zip once lowercased.
//! - yt-dlp/FFmpeg-Builds `checksums.sha256` (tag `latest`): the same
//!   `<hex>  <name>` shape as yt-dlp's, eight lines, including
//!   `ffmpeg-master-latest-{linux64,linuxarm64}-gpl.tar.xz` and
//!   `ffmpeg-master-latest-win64-gpl.zip`. The tarball holds
//!   `ffmpeg-master-latest-linux64-gpl/bin/{ffmpeg,ffprobe,ffplay}` (one xz
//!   stream, CRC64); the zip the same under `…-win64-gpl/bin/*.exe`, deflated.
//! - ffmpeg.martin-riedl.de: `GET /redirect/latest/macos/{arm64,amd64}/release/
//!   {ffmpeg,ffprobe}.zip` answers `307` with a *relative* location,
//!   `/download/macos/arm64/1789931890_9.0.2/ffmpeg.zip`. A **HEAD** to the
//!   same URL flip-flops between that `307` and a bare `404` from one request
//!   to the next, so the redirect is only ever followed with a GET. The
//!   resolved file has `<resolved URL>.sha256` beside it, one
//!   `<hex>  ffmpeg.zip` line; there is no `.sha256` beside the redirect URL
//!   itself (404). Each zip holds the single executable at its root, deflated.
//! - deno zips hold the single `deno` / `deno.exe` at their root, deflated.

use anyhow::{anyhow, Result};

use crate::models::ToolKind;

/// An OS/arch pair in `std::env::consts` spelling.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Target {
    pub os: &'static str,
    pub arch: &'static str,
}

impl Target {
    pub fn host() -> Self {
        Target { os: std::env::consts::OS, arch: std::env::consts::ARCH }
    }

    /// `name` as an executable file name on this target.
    pub fn exe(&self, name: &str) -> String {
        if self.os == "windows" {
            format!("{name}.exe")
        } else {
            name.to_string()
        }
    }

    /// The executables a managed install of `kind` puts in `bin/`.
    pub fn files(&self, kind: ToolKind) -> Vec<String> {
        match kind {
            ToolKind::Ytdlp => vec![self.exe("yt-dlp")],
            ToolKind::Ffmpeg => vec![self.exe("ffmpeg"), self.exe("ffprobe")],
            ToolKind::Deno => vec![self.exe("deno")],
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FfmpegSource {
    /// yt-dlp/FFmpeg-Builds, tag `latest`: one archive holding both binaries.
    Builds { asset: &'static str },
    /// ffmpeg.martin-riedl.de: one zip per binary. `arch` in its spelling.
    MartinRiedl { arch: &'static str },
}

/// What to download for one target.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Assets {
    /// The asset name in a yt-dlp release (and in its `SHA2-256SUMS`).
    pub ytdlp: &'static str,
    /// Deno's release asset is `deno-<triple>.zip`.
    pub deno_triple: &'static str,
    pub ffmpeg: FfmpegSource,
}

/// The managed builds for `t`, or an error naming it. `yt-dlp_macos` is a
/// universal binary, so both Mac arches share it.
pub fn assets(t: Target) -> Result<Assets> {
    let a = match (t.os, t.arch) {
        ("linux", "x86_64") => Assets {
            ytdlp: "yt-dlp_linux",
            deno_triple: "x86_64-unknown-linux-gnu",
            ffmpeg: FfmpegSource::Builds { asset: "ffmpeg-master-latest-linux64-gpl.tar.xz" },
        },
        ("linux", "aarch64") => Assets {
            ytdlp: "yt-dlp_linux_aarch64",
            deno_triple: "aarch64-unknown-linux-gnu",
            ffmpeg: FfmpegSource::Builds { asset: "ffmpeg-master-latest-linuxarm64-gpl.tar.xz" },
        },
        ("macos", "aarch64") => Assets {
            ytdlp: "yt-dlp_macos",
            deno_triple: "aarch64-apple-darwin",
            ffmpeg: FfmpegSource::MartinRiedl { arch: "arm64" },
        },
        ("macos", "x86_64") => Assets {
            ytdlp: "yt-dlp_macos",
            deno_triple: "x86_64-apple-darwin",
            ffmpeg: FfmpegSource::MartinRiedl { arch: "amd64" },
        },
        ("windows", "x86_64") => Assets {
            ytdlp: "yt-dlp.exe",
            deno_triple: "x86_64-pc-windows-msvc",
            ffmpeg: FfmpegSource::Builds { asset: "ffmpeg-master-latest-win64-gpl.zip" },
        },
        (os, arch) => return Err(anyhow!("no managed build for {os}/{arch}")),
    };
    Ok(a)
}

/// The hosts every URL is built on. Production uses [`Hosts::REAL`]; tests
/// point both at a local server so nothing touches the network.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hosts {
    pub github: String,
    pub martin_riedl: String,
}

impl Hosts {
    pub fn real() -> Self {
        Hosts {
            github: "https://github.com".into(),
            martin_riedl: "https://ffmpeg.martin-riedl.de".into(),
        }
    }
}

/// `ytdlp_channel` -> the repository its builds are published in. Anything
/// but `stable` reads as nightly, as `Settings::from_json_str` normalises it.
pub fn ytdlp_repo(channel: &str) -> &'static str {
    if channel == "stable" {
        "yt-dlp/yt-dlp"
    } else {
        "yt-dlp/yt-dlp-nightly-builds"
    }
}

pub const DENO_REPO: &str = "denoland/deno";
pub const FFMPEG_BUILDS_REPO: &str = "yt-dlp/FFmpeg-Builds";

pub fn latest_url(h: &Hosts, repo: &str) -> String {
    format!("{}/{repo}/releases/latest", h.github)
}

pub fn release_asset_url(h: &Hosts, repo: &str, tag: &str, asset: &str) -> String {
    format!("{}/{repo}/releases/download/{tag}/{asset}", h.github)
}

pub fn martin_riedl_url(h: &Hosts, arch: &str, tool: &str) -> String {
    format!("{}/redirect/latest/macos/{arch}/release/{tool}.zip", h.martin_riedl)
}

/// The tag a followed `releases/latest` landed on: the path segment after
/// `/releases/tag/`. `None` for anything else (a login wall, a 404 page).
pub fn tag_from_release_url(url: &str) -> Option<String> {
    let rest = url.split_once("/releases/tag/")?.1;
    let tag = rest.split(['?', '#', '/']).next()?.trim();
    (!tag.is_empty()).then(|| tag.to_string())
}

fn is_sha256(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit())
}

/// The hash for `name` in a `sha256sum`-style list (`<hex>  <name>` per line;
/// a `*` before the name marks binary mode and is ignored). Exact name match:
/// `yt-dlp` must not pick up `yt-dlp_linux`'s line. Lowercased.
pub fn hash_in_list(text: &str, name: &str) -> Option<String> {
    text.lines().find_map(|line| {
        let mut parts = line.split_whitespace();
        let hash = parts.next()?;
        let file = parts.next()?.trim_start_matches('*');
        (file == name && is_sha256(hash)).then(|| hash.to_ascii_lowercase())
    })
}

/// The hash in a file that describes one download: a `sha256sum` line, or
/// PowerShell's `Get-FileHash` block (deno on Windows). The first token that
/// is 64 hex digits, lowercased.
pub fn single_hash(text: &str) -> Option<String> {
    text.split(|c: char| c.is_whitespace() || c == ':')
        .find(|t| is_sha256(t))
        .map(|t| t.to_ascii_lowercase())
}

/// The version a tool's own version flag reports, from its stdout.
/// `yt-dlp --version` -> the whole first line (`2026.09.16.232951`);
/// `ffmpeg -version` -> the token after `ffmpeg version` (`n9.0.1`,
/// `N-121234-gdeadbeef-20260923`); `deno --version` -> `2.9.7` from
/// `deno 2.9.7 (stable, release, …)`.
pub fn parse_version(kind: ToolKind, stdout: &str) -> Option<String> {
    let first = stdout.lines().map(str::trim).find(|l| !l.is_empty())?;
    let v = match kind {
        ToolKind::Ytdlp => first,
        ToolKind::Ffmpeg => first.strip_prefix("ffmpeg version ")?.split_whitespace().next()?,
        ToolKind::Deno => first.strip_prefix("deno ")?.split_whitespace().next()?,
    };
    (!v.is_empty()).then(|| v.to_string())
}

/// yt-dlp's EJS solver needs Deno 2.3 or later; an older system deno is
/// treated as absent so a managed one is fetched instead.
pub const DENO_MIN: (u32, u32, u32) = (2, 3, 0);

/// `2.9.7` (or `2.9.7-rc.1`) -> `(2, 9, 7)`.
pub fn semver_triple(v: &str) -> Option<(u32, u32, u32)> {
    let core = v.trim().trim_start_matches('v').split(['-', '+']).next()?;
    let mut it = core.split('.').map(|p| p.parse::<u32>().ok());
    Some((it.next()??, it.next()??, it.next().unwrap_or(Some(0))?))
}

pub fn deno_new_enough(version: &str) -> bool {
    semver_triple(version).is_some_and(|v| v >= DENO_MIN)
}

/// Whether yt-dlp's verbose header says deno is an available JS runtime:
/// `[debug] JS runtimes: deno-2.9.6`, versus `[debug] JS runtimes: none`
/// when the path given to `--js-runtimes` is wrong. Checked against
/// yt-dlp 2026.08.19; `-v --version` prints no header at all, so the probe
/// runs with no URL and reads this line from stderr.
pub fn js_runtimes_line(output: &str) -> Option<&str> {
    output
        .lines()
        .find_map(|l| l.trim().strip_prefix("[debug] JS runtimes:"))
        .map(str::trim)
}

pub fn deno_is_a_js_runtime(output: &str) -> bool {
    js_runtimes_line(output)
        .is_some_and(|l| l.split(',').any(|r| r.trim().starts_with("deno")))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verbatim excerpt of the nightly 2026.09.16.232951 `SHA2-256SUMS`.
    const YTDLP_SUMS: &str = "\
f8ca14db511702a5dbfc5a527056312907ddd0914d0b4036f108d6849e17ef61  yt-dlp
9c900a6b5d13933b1d31a3a039139a9fa020c7fb0d8edaa049054a802641533d  yt-dlp.exe
4680fe2eca4aa3a9ea66864933f75de696d1411f432eba261f5d452ae7cef160  yt-dlp.tar.gz
69112248177f7c3a6ba7ed981e143a0214910ed6adeb40e075f08543f74a4850  yt-dlp_linux
6d32c120db479f3c9b6d91aeada2edb3cb0494cf4438b35401fb0dd69b5ff51c  yt-dlp_linux.zip
e433b968b285970c131d7e73096c133845c15f315fafd6af976e9b4d4976caf6  yt-dlp_linux_aarch64
d04fa823ed825673880ab544e74f50844c4cf1625b01b8195d6d815da96370ce  yt-dlp_macos
6abd8a1a90ec74ac4803d7d60732ab39fff5f6c71833b38c163e1a08f581dd43  yt-dlp_musllinux
";

    #[test]
    fn the_ytdlp_sums_file_is_read_by_exact_name() {
        assert_eq!(
            hash_in_list(YTDLP_SUMS, "yt-dlp_linux").as_deref(),
            Some("69112248177f7c3a6ba7ed981e143a0214910ed6adeb40e075f08543f74a4850")
        );
        assert_eq!(
            hash_in_list(YTDLP_SUMS, "yt-dlp").as_deref(),
            Some("f8ca14db511702a5dbfc5a527056312907ddd0914d0b4036f108d6849e17ef61")
        );
        assert_eq!(
            hash_in_list(YTDLP_SUMS, "yt-dlp.exe").as_deref(),
            Some("9c900a6b5d13933b1d31a3a039139a9fa020c7fb0d8edaa049054a802641533d")
        );
        assert_eq!(hash_in_list(YTDLP_SUMS, "yt-dlp_win_arm64.zip"), None);
        assert_eq!(hash_in_list(YTDLP_SUMS, "linux"), None);
    }

    #[test]
    fn a_binary_mode_star_and_uppercase_hash_still_match() {
        let text = "ABCDEF0123456789ABCDEF0123456789ABCDEF0123456789ABCDEF0123456789 *file.zip\r\n";
        assert_eq!(
            hash_in_list(text, "file.zip").as_deref(),
            Some("abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789")
        );
    }

    #[test]
    fn a_line_with_a_short_hash_is_not_a_match() {
        assert_eq!(hash_in_list("deadbeef  yt-dlp_linux\n", "yt-dlp_linux"), None);
    }

    #[test]
    fn the_ffmpeg_builds_checksums_name_each_asset() {
        // Verbatim excerpt of FFmpeg-Builds `latest` checksums.sha256.
        let text = "\
a887cb7990ebecb4c9de54b985433a1f457c9506fb1f5a45573ccdfd2c46f466  ffmpeg-master-latest-win64-gpl-shared.zip
5e21ef1d5661239098129224e4b56c201bcf833f821ecf03b823c758fd14bb01  ffmpeg-master-latest-win64-gpl.zip
834cb104b253bbc26c2bc4c1dd75311846f8f94419710a9d67d6de2832634a02  ffmpeg-master-latest-linux64-gpl.tar.xz
ff43689725104fe47ac67a2b0c0babb5375ed7e39d4fe17363a7b206cd6aa8b7  ffmpeg-master-latest-linuxarm64-gpl.tar.xz
";
        assert_eq!(
            hash_in_list(text, "ffmpeg-master-latest-win64-gpl.zip").as_deref(),
            Some("5e21ef1d5661239098129224e4b56c201bcf833f821ecf03b823c758fd14bb01")
        );
        assert_eq!(
            hash_in_list(text, "ffmpeg-master-latest-linux64-gpl.tar.xz").as_deref(),
            Some("834cb104b253bbc26c2bc4c1dd75311846f8f94419710a9d67d6de2832634a02")
        );
    }

    #[test]
    fn a_deno_sha256sum_is_read_in_both_its_shapes() {
        let unix = "c6527f24f4b16031d3ae4fa9f658d5f11534c8d84ce7dc8502420280919c3490  deno-x86_64-unknown-linux-gnu.zip\n";
        assert_eq!(
            single_hash(unix).as_deref(),
            Some("c6527f24f4b16031d3ae4fa9f658d5f11534c8d84ce7dc8502420280919c3490")
        );
        // Verbatim, CRLF and all, from deno-x86_64-pc-windows-msvc.zip.sha256sum.
        let windows = "\r\nAlgorithm : SHA256\r\nHash      : A0C3101B4158D1DFB7D6A78A7BF0F3DE80C96BB423C152BEEC8BEB22786F2238\r\nPath      : C:\\a\\deno\\deno\\target\\release\\deno-x86_64-pc-windows-msvc.zip\r\n\r\n";
        assert_eq!(
            single_hash(windows).as_deref(),
            Some("a0c3101b4158d1dfb7d6a78a7bf0f3de80c96bb423c152beec8beb22786f2238")
        );
        assert_eq!(single_hash("<!DOCTYPE html><html>Not Found</html>"), None);
    }

    #[test]
    fn a_martin_riedl_sha256_is_one_line() {
        let text = "c8ed4c4e6978a03c485edbfe4e0a5dc2380f8a30bba5150531b31b094492d924  ffmpeg.zip\n";
        assert_eq!(
            single_hash(text).as_deref(),
            Some("c8ed4c4e6978a03c485edbfe4e0a5dc2380f8a30bba5150531b31b094492d924")
        );
    }

    #[test]
    fn every_supported_target_has_all_three_sources() {
        let t = |os, arch| assets(Target { os, arch }).unwrap();
        assert_eq!(
            t("linux", "x86_64"),
            Assets {
                ytdlp: "yt-dlp_linux",
                deno_triple: "x86_64-unknown-linux-gnu",
                ffmpeg: FfmpegSource::Builds { asset: "ffmpeg-master-latest-linux64-gpl.tar.xz" },
            }
        );
        assert_eq!(
            t("linux", "aarch64"),
            Assets {
                ytdlp: "yt-dlp_linux_aarch64",
                deno_triple: "aarch64-unknown-linux-gnu",
                ffmpeg: FfmpegSource::Builds { asset: "ffmpeg-master-latest-linuxarm64-gpl.tar.xz" },
            }
        );
        assert_eq!(
            t("macos", "aarch64"),
            Assets {
                ytdlp: "yt-dlp_macos",
                deno_triple: "aarch64-apple-darwin",
                ffmpeg: FfmpegSource::MartinRiedl { arch: "arm64" },
            }
        );
        assert_eq!(
            t("macos", "x86_64"),
            Assets {
                ytdlp: "yt-dlp_macos",
                deno_triple: "x86_64-apple-darwin",
                ffmpeg: FfmpegSource::MartinRiedl { arch: "amd64" },
            }
        );
        assert_eq!(
            t("windows", "x86_64"),
            Assets {
                ytdlp: "yt-dlp.exe",
                deno_triple: "x86_64-pc-windows-msvc",
                ffmpeg: FfmpegSource::Builds { asset: "ffmpeg-master-latest-win64-gpl.zip" },
            }
        );
    }

    #[test]
    fn an_unknown_target_names_itself_in_the_error() {
        for (os, arch) in [("windows", "aarch64"), ("freebsd", "x86_64"), ("linux", "arm")] {
            let err = assets(Target { os, arch }).unwrap_err().to_string();
            assert_eq!(err, format!("no managed build for {os}/{arch}"));
        }
    }

    #[test]
    fn the_host_is_a_supported_target() {
        // CI builds exactly the targets the table knows; a host outside it
        // would fail every install at runtime instead of here.
        assert!(assets(Target::host()).is_ok(), "{:?}", Target::host());
    }

    #[test]
    fn executables_carry_exe_only_on_windows() {
        let win = Target { os: "windows", arch: "x86_64" };
        let mac = Target { os: "macos", arch: "aarch64" };
        assert_eq!(win.files(ToolKind::Ffmpeg), vec!["ffmpeg.exe", "ffprobe.exe"]);
        assert_eq!(mac.files(ToolKind::Ffmpeg), vec!["ffmpeg", "ffprobe"]);
        assert_eq!(win.files(ToolKind::Ytdlp), vec!["yt-dlp.exe"]);
        assert_eq!(mac.files(ToolKind::Deno), vec!["deno"]);
    }

    #[test]
    fn urls_are_built_exactly_as_the_publishers_serve_them() {
        let h = Hosts::real();
        assert_eq!(
            release_asset_url(&h, ytdlp_repo("nightly"), "2026.09.16.232951", "yt-dlp_linux"),
            "https://github.com/yt-dlp/yt-dlp-nightly-builds/releases/download/2026.09.16.232951/yt-dlp_linux"
        );
        assert_eq!(
            release_asset_url(&h, ytdlp_repo("stable"), "2026.08.19", "SHA2-256SUMS"),
            "https://github.com/yt-dlp/yt-dlp/releases/download/2026.08.19/SHA2-256SUMS"
        );
        assert_eq!(ytdlp_repo("anything else"), "yt-dlp/yt-dlp-nightly-builds");
        assert_eq!(
            latest_url(&h, DENO_REPO),
            "https://github.com/denoland/deno/releases/latest"
        );
        assert_eq!(
            release_asset_url(&h, FFMPEG_BUILDS_REPO, "latest", "checksums.sha256"),
            "https://github.com/yt-dlp/FFmpeg-Builds/releases/download/latest/checksums.sha256"
        );
        assert_eq!(
            martin_riedl_url(&h, "arm64", "ffprobe"),
            "https://ffmpeg.martin-riedl.de/redirect/latest/macos/arm64/release/ffprobe.zip"
        );
    }

    #[test]
    fn the_latest_redirect_yields_the_tag() {
        assert_eq!(
            tag_from_release_url("https://github.com/yt-dlp/yt-dlp/releases/tag/2026.08.19").as_deref(),
            Some("2026.08.19")
        );
        assert_eq!(
            tag_from_release_url("https://github.com/denoland/deno/releases/tag/v2.9.7?x=1").as_deref(),
            Some("v2.9.7")
        );
        assert_eq!(tag_from_release_url("https://github.com/yt-dlp/yt-dlp/releases"), None);
        assert_eq!(tag_from_release_url("https://github.com/login"), None);
    }

    #[test]
    fn versions_are_read_from_each_tools_own_output() {
        assert_eq!(parse_version(ToolKind::Ytdlp, "2026.09.16.232951\n").as_deref(), Some("2026.09.16.232951"));
        assert_eq!(
            parse_version(ToolKind::Ffmpeg, "ffmpeg version n9.0.1 Copyright (c) 2000-2026 the FFmpeg developers\nbuilt with gcc\n").as_deref(),
            Some("n9.0.1")
        );
        assert_eq!(
            parse_version(ToolKind::Ffmpeg, "ffmpeg version N-121234-gdeadbeef-20260923 Copyright (c) 2000-2026\n").as_deref(),
            Some("N-121234-gdeadbeef-20260923")
        );
        assert_eq!(
            parse_version(ToolKind::Deno, "deno 2.9.6 (stable, release, x86_64-unknown-linux-gnu)\nv8 14.0\ntypescript 5.9\n").as_deref(),
            Some("2.9.6")
        );
        assert_eq!(parse_version(ToolKind::Deno, "node v22\n"), None);
        assert_eq!(parse_version(ToolKind::Ytdlp, "\n\n"), None);
    }

    #[test]
    fn a_deno_older_than_2_3_is_not_new_enough() {
        assert!(deno_new_enough("2.3.0"));
        assert!(deno_new_enough("2.9.7"));
        assert!(deno_new_enough("3.0.0-rc.1"));
        assert!(deno_new_enough("10.0.0"));
        assert!(!deno_new_enough("2.2.12"));
        assert!(!deno_new_enough("1.46.3"));
        assert!(!deno_new_enough("garbage"));
    }

    #[test]
    fn the_js_runtimes_line_decides_whether_deno_was_found() {
        let found = "[debug] Optional libraries: yt_dlp_ejs-0.8.0\n[debug] JS runtimes: deno-2.9.6\n[debug] Proxy map: {}\n";
        let none = "[debug] JS runtimes: none\n";
        assert!(deno_is_a_js_runtime(found));
        assert_eq!(js_runtimes_line(found), Some("deno-2.9.6"));
        assert!(!deno_is_a_js_runtime(none));
        assert!(!deno_is_a_js_runtime("yt-dlp: error: You must provide at least one URL.\n"));
        assert!(deno_is_a_js_runtime("[debug] JS runtimes: node-22.1.0, deno-2.9.7\n"));
    }
}
