//! A cookies file in whatever shape it was exported, turned into the one shape
//! yt-dlp reads.
//!
//! yt-dlp's `--cookies` takes only a Netscape cookies.txt, and is strict about
//! it (yt_dlp/cookies.py `YoutubeDLCookieJar.load`, CPython's
//! `MozillaCookieJar._really_load`, both read 2026-10-03): the first line must
//! be the `# Netscape HTTP Cookie File` magic, an entry of anything but seven
//! tab-separated fields is dropped, and the second field must be TRUE exactly
//! when the domain starts with a dot -- CPython *asserts* that, and one line
//! that breaks it fails the whole file. A JSON file is refused by name.
//!
//! Exports come just as often as JSON (Cookie-Editor, EditThisCookie, Cookie
//! Quick Manager, Playwright's storageState, Selenium's `get_cookies`), and a
//! `Cookie:` header copied out of a browser's network panel is `name=value;
//! name=value`. So MyTube never hands the chosen file to yt-dlp: [`prepare`]
//! parses it, writes a canonical copy under the config dir, and yt-dlp reads
//! that. The user's own file is never written to, where before yt-dlp
//! rewrote it at the end of every run.
//!
//! The copy is named for the source's SHA-256 and kept while the source is
//! unchanged, because yt-dlp saves back the cookies YouTube refreshed during a
//! run into the file it was given, and regenerating the copy every time would
//! throw those away. A re-exported source is a new name and a fresh copy.

use anyhow::{anyhow, bail, Context, Result};
use serde::Serialize;
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

/// Where a cookie with no domain of its own is sent. A `name=value` list and a
/// flat JSON object carry none, and YouTube is the only site MyTube talks to.
const DEFAULT_DOMAIN: &str = ".youtube.com";

/// The cookies YouTube sets on `youtube.com` only for a signed-in session.
/// `LOGIN_INFO` is YouTube's own; the `SAPISID` family is what yt-dlp hashes
/// into its authorisation header. A signed-out browser holds none of them.
pub const YOUTUBE_LOGIN_COOKIES: &[&str] = &[
    "LOGIN_INFO",
    "SAPISID",
    "__Secure-1PAPISID",
    "__Secure-3PAPISID",
    "__Secure-1PSID",
    "__Secure-3PSID",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cookie {
    /// A leading dot means the cookie also goes to subdomains.
    pub domain: String,
    pub path: String,
    pub secure: bool,
    /// Seconds since the epoch; 0 is a session cookie, which yt-dlp keeps.
    pub expires: u64,
    pub name: String,
    pub value: String,
}

impl Cookie {
    fn on_youtube(&self) -> bool {
        let d = self.domain.trim_start_matches('.').to_ascii_lowercase();
        d == "youtube.com" || d.ends_with(".youtube.com")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Netscape,
    Json,
    /// `name=value; name=value`, as a `Cookie:` header or one pair per line.
    Pairs,
}

impl Format {
    pub fn label(self) -> &'static str {
        match self {
            Format::Netscape => "Netscape cookies.txt",
            Format::Json => "JSON cookie export",
            Format::Pairs => "name=value list",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Parsed {
    pub format: Format,
    pub cookies: Vec<Cookie>,
    /// Entries that were there but could not be read, skipped the way yt-dlp
    /// skips a bad line rather than refusing the file.
    pub skipped: usize,
}

/// What the Settings view says about a chosen file.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CookiesFileInfo {
    /// [`Format::label`].
    pub format: String,
    pub cookies: usize,
    pub skipped: usize,
    /// Whether any of them is for youtube.com at all.
    pub youtube: bool,
    /// Whether they include a YouTube sign-in ([`YOUTUBE_LOGIN_COOKIES`]).
    pub signed_in: bool,
}

const UNRECOGNISED: &str = "it is not a Netscape cookies.txt, a JSON cookie export, or a list of \
    name=value pairs";

/// Reads cookies out of `text`, whichever of the three shapes it is in. An
/// `Err` is one sentence saying why nothing usable came out.
pub fn parse(text: &str) -> Result<Parsed> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let trimmed = text.trim();
    if trimmed.is_empty() {
        bail!("it is empty");
    }
    let parsed = if trimmed.starts_with(['[', '{']) {
        parse_json(trimmed)?
    } else if let Some(p) = parse_netscape(text) {
        p
    } else if let Some(p) = parse_pairs(trimmed) {
        p
    } else {
        bail!("{UNRECOGNISED}");
    };
    if parsed.cookies.is_empty() {
        match parsed.skipped {
            0 => bail!("it holds no cookies"),
            n => bail!("none of its {n} entries could be read as a cookie"),
        }
    }
    Ok(parsed)
}

/// The cookies as a file yt-dlp loads without a warning.
pub fn to_netscape(cookies: &[Cookie]) -> String {
    let mut out = String::from(
        "# Netscape HTTP Cookie File\n\
         # Written by MyTube from the cookies file chosen in Settings; yt-dlp updates it.\n\n",
    );
    let flag = |b: bool| if b { "TRUE" } else { "FALSE" };
    for c in cookies {
        out.push_str(&format!(
            "{}\t{}\t{}\t{}\t{}\t{}\t{}\n",
            c.domain,
            flag(c.domain.starts_with('.')),
            c.path,
            flag(c.secure),
            c.expires,
            c.name,
            c.value,
        ));
    }
    out
}

/// What [`inspect`] and [`prepare`] both start from: the file read, decoded
/// and parsed, with any failure worded for the person who chose it.
fn load(source: &Path) -> Result<(Parsed, Vec<u8>)> {
    let name = source.file_name().map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| source.display().to_string());
    let bytes = std::fs::read(source).map_err(|e| match e.kind() {
        std::io::ErrorKind::NotFound => anyhow!("The cookies file {} no longer exists.", source.display()),
        _ => anyhow!("The cookies file {} can't be read: {e}", source.display()),
    })?;
    let parsed = decode(&bytes)
        .and_then(|text| parse(&text))
        .map_err(|why| anyhow!("{name} can't be used as a cookies file: {why}."))?;
    Ok((parsed, bytes))
}

/// The Settings view's check of a file, before it is saved and after.
pub fn inspect(source: &Path) -> Result<CookiesFileInfo> {
    let (p, _) = load(source)?;
    Ok(CookiesFileInfo {
        format: p.format.label().into(),
        cookies: p.cookies.len(),
        skipped: p.skipped,
        youtube: p.cookies.iter().any(Cookie::on_youtube),
        signed_in: p.cookies.iter().any(|c| c.on_youtube() && YOUTUBE_LOGIN_COOKIES.contains(&c.name.as_str())),
    })
}

/// The Netscape copy of `source` that yt-dlp is given, written into `store`
/// unless one for this exact content is already there and still loads.
pub fn prepare(source: &Path, store: &Path) -> Result<PathBuf> {
    let bytes = std::fs::read(source).ok();
    if let Some(b) = &bytes {
        let copy = store.join(copy_name(b));
        let intact = std::fs::read_to_string(&copy)
            .is_ok_and(|t| parse(&t).is_ok_and(|p| p.format == Format::Netscape));
        if intact {
            return Ok(copy);
        }
    }
    // Read again through `load`, so a missing or unreadable file fails with
    // the same words the Settings view shows.
    let (parsed, bytes) = load(source)?;
    if parsed.skipped > 0 {
        eprintln!("cookies: {} skipped {} unreadable entries", source.display(), parsed.skipped);
    }
    let copy = store.join(copy_name(&bytes));
    write_private(store, &copy, &to_netscape(&parsed.cookies))
        .with_context(|| format!("writing {}", copy.display()))?;
    sweep(store, &copy);
    Ok(copy)
}

/// Where the copies live: beside settings.json, machine-local like the
/// `cookies_file` setting itself, and never part of an export.
pub fn store_dir() -> PathBuf {
    crate::config::config_dir().join("cookies")
}

fn copy_name(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let hex: String = digest.iter().take(8).map(|b| format!("{b:02x}")).collect();
    format!("{hex}.txt")
}

/// `text` at `dest`, readable by this user alone: it is a signed-in session.
/// Written beside it and renamed over it, so a yt-dlp starting meanwhile never
/// reads half a file.
fn write_private(store: &Path, dest: &Path, text: &str) -> Result<()> {
    use std::io::Write;
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEQ: AtomicU64 = AtomicU64::new(0);

    std::fs::create_dir_all(store)?;
    let tmp = store.join(format!(
        ".{}.{}.{}.partial",
        dest.file_name().unwrap_or_default().to_string_lossy(),
        std::process::id(),
        SEQ.fetch_add(1, Ordering::Relaxed),
    ));
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
        opts.mode(0o600);
        let _ = std::fs::set_permissions(store, std::fs::Permissions::from_mode(0o700));
    }
    let result = (|| {
        let mut f = opts.open(&tmp)?;
        f.write_all(text.as_bytes())?;
        f.sync_all()?;
        drop(f);
        std::fs::rename(&tmp, dest)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    Ok(result?)
}

/// Copies of earlier sources: a session nobody chose any more should not sit
/// on disk. A yt-dlp still running from one recreates it when it exits, and
/// the next prepare removes it again.
fn sweep(store: &Path, keep: &Path) {
    let Ok(rd) = std::fs::read_dir(store) else { return };
    for entry in rd.flatten() {
        let p = entry.path();
        if p != keep && p.extension().is_some_and(|e| e == "txt") {
            let _ = std::fs::remove_file(p);
        }
    }
}

/// UTF-8, or UTF-16 with a byte-order mark -- what PowerShell's `>` writes.
fn decode(bytes: &[u8]) -> Result<String> {
    let utf16 = |rest: &[u8], unit: fn([u8; 2]) -> u16| {
        let units: Vec<u16> = rest.chunks_exact(2).map(|c| unit([c[0], c[1]])).collect();
        String::from_utf16(&units).map_err(|_| anyhow!("it is not readable text"))
    };
    if let Some(rest) = bytes.strip_prefix(&[0xFF, 0xFE]) {
        return utf16(rest, u16::from_le_bytes);
    }
    if let Some(rest) = bytes.strip_prefix(&[0xFE, 0xFF]) {
        return utf16(rest, u16::from_be_bytes);
    }
    if bytes.starts_with(b"SQLite format 3\0") {
        bail!("it is a browser's own cookie database, not an exported cookies file -- \
               choose that browser in the list instead");
    }
    String::from_utf8(bytes.to_vec()).map_err(|_| anyhow!("it is not a text file"))
}

// ---------------------------------------------------------------- Netscape

/// `None` when nothing in `text` reads as Netscape at all -- no magic line and
/// no entry -- so the next shape gets its turn.
fn parse_netscape(text: &str) -> Option<Parsed> {
    let mut magic = false;
    let mut cookies = Vec::new();
    let mut skipped = 0;
    for raw in text.lines() {
        let line = match raw.strip_prefix("#HttpOnly_") {
            Some(rest) => rest,
            None => {
                let t = raw.trim();
                if t.starts_with('#') {
                    magic |= t.to_ascii_lowercase().contains("http cookie file");
                    continue;
                }
                if t.is_empty() {
                    continue;
                }
                raw
            }
        };
        match netscape_entry(line.trim_end_matches('\r')) {
            Some(c) => cookies.push(c),
            None => skipped += 1,
        }
    }
    (magic || !cookies.is_empty()).then_some(Parsed { format: Format::Netscape, cookies, skipped })
}

/// One entry: seven tab-separated fields, or six when an exporter dropped an
/// empty value. Some exporters separate with spaces; that is taken too, since
/// no field of a valid entry can contain one.
fn netscape_entry(line: &str) -> Option<Cookie> {
    let mut fields: Vec<&str> = line.split('\t').collect();
    if !(6..=7).contains(&fields.len()) {
        fields = line.split_whitespace().collect();
        if !(6..=7).contains(&fields.len()) {
            return None;
        }
    }
    let subdomains = parse_bool(fields[1])?;
    let secure = parse_bool(fields[3])?;
    let expires = match fields[4].trim() {
        "" => 0,
        t => expiry(t.parse::<f64>().ok()?),
    };
    let value = fields.get(6).copied().unwrap_or("");
    cookie(Some(fields[0]), Some(subdomains), fields[2], secure, expires, fields[5], value)
}

// -------------------------------------------------------------------- JSON

fn parse_json(text: &str) -> Result<Parsed> {
    let v: Value = serde_json::from_str(text)
        .map_err(|e| anyhow!("it looks like JSON but does not parse ({e})"))?;
    let not_cookies = || anyhow!("it is JSON, but not a list of cookies");
    let items: Vec<&Value> = match &v {
        Value::Array(a) => a.iter().collect(),
        Value::Object(o) => {
            if let Some(Value::Array(a)) = field(o, &["cookies"]) {
                // Playwright's storageState, and exporters that wrap the list.
                a.iter().collect()
            } else if field(o, NAME).is_some() && field(o, VALUE).is_some() {
                vec![&v]
            } else if !o.is_empty() && o.values().all(Value::is_string) {
                // `{"SID": "…", "HSID": "…"}`: names and values, nothing else.
                let mut cookies = Vec::new();
                let mut skipped = 0;
                for (name, value) in o {
                    match cookie(None, None, "/", true, 0, name, value.as_str().unwrap_or_default()) {
                        Some(c) => cookies.push(c),
                        None => skipped += 1,
                    }
                }
                return Ok(Parsed { format: Format::Json, cookies, skipped });
            } else {
                return Err(not_cookies());
            }
        }
        _ => return Err(not_cookies()),
    };
    let mut cookies = Vec::new();
    let mut skipped = 0;
    for item in items {
        match item.as_object().and_then(json_cookie) {
            Some(c) => cookies.push(c),
            None => skipped += 1,
        }
    }
    Ok(Parsed { format: Format::Json, cookies, skipped })
}

/// Every exporter names the same seven things differently. These are the
/// names Chrome's and Firefox's extension APIs use (Cookie-Editor,
/// EditThisCookie, Get cookies.txt), Playwright and Puppeteer, Selenium, and
/// Cookie Quick Manager's `… raw` keys; matched without regard to case.
const NAME: &[&str] = &["name", "Name raw"];
const VALUE: &[&str] = &["value", "Content raw", "content"];
const DOMAIN: &[&str] = &["domain", "host", "Host raw", "host_key"];
const PATH: &[&str] = &["path", "Path raw"];
const SECURE: &[&str] = &["secure", "isSecure", "Send for raw"];
const HOST_ONLY: &[&str] = &["hostOnly", "This domain only raw"];
const EXPIRES: &[&str] = &["expirationDate", "expires", "expiry", "Expires raw"];
const SESSION: &[&str] = &["session"];

fn json_cookie(o: &Map<String, Value>) -> Option<Cookie> {
    let name = field(o, NAME).and_then(text)?;
    let value = field(o, VALUE).and_then(text).unwrap_or_default();
    let domain = field(o, DOMAIN).and_then(text);
    let path = field(o, PATH).and_then(text).unwrap_or_default();
    let secure = field(o, SECURE).and_then(truth).unwrap_or(false);
    let host_only = field(o, HOST_ONLY).and_then(truth);
    let session = field(o, SESSION).and_then(truth).unwrap_or(false);
    let expires = if session { 0 } else { field(o, EXPIRES).and_then(seconds).map(expiry).unwrap_or(0) };
    cookie(domain.as_deref(), host_only.map(|h| !h), &path, secure, expires, &name, &value)
}

fn field<'a>(o: &'a Map<String, Value>, names: &[&str]) -> Option<&'a Value> {
    o.iter().find(|(k, _)| names.iter().any(|n| k.eq_ignore_ascii_case(n))).map(|(_, v)| v)
}

fn text(v: &Value) -> Option<String> {
    match v {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        Value::Bool(b) => Some(b.to_string()),
        _ => None,
    }
}

fn truth(v: &Value) -> Option<bool> {
    match v {
        Value::Bool(b) => Some(*b),
        Value::Number(n) => n.as_f64().map(|f| f != 0.0),
        Value::String(s) => parse_bool(s),
        _ => None,
    }
}

fn seconds(v: &Value) -> Option<f64> {
    match v {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => s.trim().parse().ok(),
        _ => None,
    }
}

// ---------------------------------------------------------- name=value list

/// `Cookie: a=1; b=2`, or the same pairs one per line. Every non-empty piece
/// has to be a pair, or this is not the shape and nothing is taken from it.
fn parse_pairs(text: &str) -> Option<Parsed> {
    let body = match text.get(..7) {
        Some(head) if head.eq_ignore_ascii_case("cookie:") => &text[7..],
        _ => text,
    };
    let mut cookies = Vec::new();
    for piece in body.split([';', '\n']) {
        let piece = piece.trim();
        if piece.is_empty() {
            continue;
        }
        let (name, value) = piece.split_once('=')?;
        let name = name.trim();
        if name.is_empty() || name.contains(char::is_whitespace) {
            return None;
        }
        cookies.push(cookie(None, None, "/", true, 0, name, value.trim())?);
    }
    (!cookies.is_empty()).then_some(Parsed { format: Format::Pairs, cookies, skipped: 0 })
}

// ------------------------------------------------------------------ shared

/// One cookie, normalised for a Netscape line, or `None` if it cannot be one.
/// `domain: None` is a format that carries no domain at all, which means
/// YouTube; `subdomains` is the Netscape flag / the inverse of JSON's
/// `hostOnly`, where the format has one.
fn cookie(
    domain: Option<&str>,
    subdomains: Option<bool>,
    path: &str,
    secure: bool,
    expires: u64,
    name: &str,
    value: &str,
) -> Option<Cookie> {
    let mut domain = match domain {
        None => DEFAULT_DOMAIN.to_string(),
        // Cookie Quick Manager writes the host as a URL: `https://.youtube.com/`.
        Some(d) => {
            let d = d.trim();
            let d = d.strip_prefix("https://").or_else(|| d.strip_prefix("http://")).unwrap_or(d);
            d.trim_end_matches('/').to_string()
        }
    };
    // The leading dot is what yt-dlp writes the flag from, so the two cannot
    // disagree in the copy. A flag asking for subdomains on a dotless domain
    // gets its dot; a dotted domain stays a domain cookie whatever the flag.
    if subdomains == Some(true) && !domain.starts_with('.') {
        domain.insert(0, '.');
    }
    let bad = |s: &str| s.chars().any(|c| c.is_control());
    if domain.trim_start_matches('.').is_empty() || domain.contains(char::is_whitespace) || bad(&domain) {
        return None;
    }
    if name.is_empty() || name.contains(char::is_whitespace) || bad(name) || bad(value) || bad(path) {
        return None;
    }
    let path = if path.trim().is_empty() { "/" } else { path.trim() };
    // A prefixed name is only ever set by a secure origin, and a browser
    // refuses to store one otherwise; an export that lost the flag is wrong.
    let secure = secure || name.starts_with("__Secure-") || name.starts_with("__Host-");
    Some(Cookie {
        domain,
        path: path.to_string(),
        secure,
        expires,
        name: name.to_string(),
        value: value.to_string(),
    })
}

fn parse_bool(s: &str) -> Option<bool> {
    match s.trim().to_ascii_lowercase().as_str() {
        "true" | "1" | "yes" => Some(true),
        "false" | "0" | "no" => Some(false),
        _ => None,
    }
}

/// Whole seconds since the epoch. Zero or less is a session cookie; anything
/// past the year 5000 is milliseconds, which some exporters write.
fn expiry(secs: f64) -> u64 {
    if !secs.is_finite() || secs <= 0.0 {
        0
    } else if secs > 1e11 {
        (secs / 1000.0) as u64
    } else {
        secs as u64
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn c(domain: &str, secure: bool, expires: u64, name: &str, value: &str) -> Cookie {
        Cookie { domain: domain.into(), path: "/".into(), secure, expires, name: name.into(), value: value.into() }
    }

    fn ok(text: &str) -> Parsed {
        parse(text).unwrap_or_else(|e| panic!("{e}: {text:?}"))
    }

    // -- Netscape

    #[test]
    fn netscape_reads_as_yt_dlp_writes_it() {
        let p = ok("# Netscape HTTP Cookie File\n\
                    # This file is generated by yt-dlp.  Do not edit.\n\n\
                    .youtube.com\tTRUE\t/\tTRUE\t1790000000\tSID\tabc\n\
                    www.youtube.com\tFALSE\t/feed\tFALSE\t0\tPREF\tf6=8\n");
        assert_eq!(p.format, Format::Netscape);
        assert_eq!(p.skipped, 0);
        assert_eq!(p.cookies, vec![
            c(".youtube.com", true, 1790000000, "SID", "abc"),
            Cookie { path: "/feed".into(), ..c("www.youtube.com", false, 0, "PREF", "f6=8") },
        ]);
    }

    #[test]
    fn netscape_tolerates_what_exporters_get_wrong() {
        let p = ok("\u{feff}#HttpOnly_.youtube.com\tTRUE\t/\tTRUE\t1790000000.75\tLOGIN_INFO\tx\r\n\
                    youtube.com TRUE / true 1790000000 VISITOR_INFO1_LIVE y\r\n\
                    .youtube.com\tTRUE\t/\tFALSE\t\tYSC\n\
                    .youtube.com\tFALSE\t/\tFALSE\t-1\t__Secure-3PSID\tz\n");
        assert_eq!(p.cookies, vec![
            c(".youtube.com", true, 1790000000, "LOGIN_INFO", "x"),
            // TRUE on a dotless domain: the dot is added, since yt-dlp's
            // loader asserts the two agree.
            c(".youtube.com", true, 1790000000, "VISITOR_INFO1_LIVE", "y"),
            // Six fields: the empty value was dropped, and an empty expiry
            // is a session cookie.
            c(".youtube.com", false, 0, "YSC", ""),
            // A `__Secure-` name is secure whatever the file says.
            c(".youtube.com", true, 0, "__Secure-3PSID", "z"),
        ]);
    }

    #[test]
    fn a_bad_netscape_line_is_skipped_not_fatal() {
        let p = ok("# Netscape HTTP Cookie File\n\
                    .youtube.com\tTRUE\t/\tTRUE\tnever\tA\t1\n\
                    garbage\n\
                    .youtube.com\tTRUE\t/\tTRUE\t0\tB\t2\n");
        assert_eq!(p.cookies.len(), 1);
        assert_eq!(p.skipped, 2);
    }

    #[test]
    fn a_netscape_header_with_nothing_readable_says_so() {
        let e = parse("# Netscape HTTP Cookie File\nnot a cookie\n").unwrap_err().to_string();
        assert_eq!(e, "none of its 1 entries could be read as a cookie");
        let e = parse("# Netscape HTTP Cookie File\n# nothing here\n").unwrap_err().to_string();
        assert_eq!(e, "it holds no cookies");
    }

    // -- JSON

    #[test]
    fn json_from_a_browser_extension() {
        // Cookie-Editor / EditThisCookie: Chrome's cookies API, verbatim.
        let p = ok(r#"[
            {"domain": ".youtube.com", "expirationDate": 1790000000.123, "hostOnly": false,
             "httpOnly": true, "name": "SAPISID", "path": "/", "sameSite": "unspecified",
             "secure": true, "session": false, "storeId": "0", "value": "s/1"},
            {"domain": "www.youtube.com", "hostOnly": true, "name": "PREF", "path": "/",
             "secure": false, "session": true, "value": "tz=UTC"},
            {"domain": "youtube.com", "hostOnly": false, "name": "YSC", "value": "q",
             "expirationDate": 1790000000}
        ]"#);
        assert_eq!(p.format, Format::Json);
        assert_eq!(p.cookies, vec![
            c(".youtube.com", true, 1790000000, "SAPISID", "s/1"),
            c("www.youtube.com", false, 0, "PREF", "tz=UTC"),
            c(".youtube.com", false, 1790000000, "YSC", "q"),
        ]);
    }

    #[test]
    fn json_from_playwright_puppeteer_and_selenium() {
        let playwright = ok(r#"{"cookies": [{"name": "SID", "value": "a", "domain": ".youtube.com",
            "path": "/", "expires": -1, "httpOnly": false, "secure": true, "sameSite": "Lax"}],
            "origins": []}"#);
        assert_eq!(playwright.cookies, vec![c(".youtube.com", true, 0, "SID", "a")]);

        let selenium = ok(r#"[{"name": "SID", "value": "a", "domain": ".youtube.com",
            "path": "/", "expiry": 1790000000, "secure": true, "httpOnly": true}]"#);
        assert_eq!(selenium.cookies, vec![c(".youtube.com", true, 1790000000, "SID", "a")]);

        // Milliseconds, which some tools write.
        let ms = ok(r#"[{"name": "SID", "value": "a", "domain": ".youtube.com", "expires": 1790000000000}]"#);
        assert_eq!(ms.cookies[0].expires, 1790000000);
    }

    #[test]
    fn json_from_cookie_quick_manager() {
        let p = ok(r#"[{"Host raw": "https://.youtube.com/", "Name raw": "SID", "Path raw": "/",
            "Content raw": "a", "Expires raw": "1790000000", "Send for raw": "true",
            "HTTP only raw": "false", "This domain only raw": "false"}]"#);
        assert_eq!(p.cookies, vec![c(".youtube.com", true, 1790000000, "SID", "a")]);
    }

    #[test]
    fn json_as_a_flat_map_or_a_single_cookie() {
        let mut flat = ok(r#"{"SID": "a", "HSID": "b"}"#).cookies;
        flat.sort_by(|a, b| a.name.cmp(&b.name));
        assert_eq!(flat, vec![
            c(".youtube.com", true, 0, "HSID", "b"),
            c(".youtube.com", true, 0, "SID", "a"),
        ]);
        let one = ok(r#"{"name": "SID", "value": "a", "domain": ".youtube.com"}"#);
        assert_eq!(one.cookies, vec![c(".youtube.com", false, 0, "SID", "a")]);
    }

    #[test]
    fn json_entries_without_a_name_are_skipped() {
        let p = ok(r#"[{"name": "SID", "value": "a"}, {"value": "orphan"}, "text", {"name": "has space", "value": "x"}]"#);
        assert_eq!(p.cookies, vec![c(".youtube.com", false, 0, "SID", "a")]);
        assert_eq!(p.skipped, 3);
    }

    #[test]
    fn json_that_is_not_cookies_is_refused_with_a_reason() {
        let e = parse(r#"{"version": 2, "theme": {"dark": true}}"#).unwrap_err().to_string();
        assert_eq!(e, "it is JSON, but not a list of cookies");
        let e = parse("[{\"name\": ").unwrap_err().to_string();
        assert!(e.starts_with("it looks like JSON but does not parse"), "{e}");
        assert_eq!(parse("[]").unwrap_err().to_string(), "it holds no cookies");
    }

    // -- name=value

    #[test]
    fn a_cookie_header_copied_from_devtools() {
        let p = ok("Cookie: SID=a; HSID=b;  PREF=f6=40000000&tz=UTC ;LOGIN_INFO=AFmmF2s:QUQ3\n");
        assert_eq!(p.format, Format::Pairs);
        assert_eq!(p.cookies, vec![
            c(".youtube.com", true, 0, "SID", "a"),
            c(".youtube.com", true, 0, "HSID", "b"),
            c(".youtube.com", true, 0, "PREF", "f6=40000000&tz=UTC"),
            c(".youtube.com", true, 0, "LOGIN_INFO", "AFmmF2s:QUQ3"),
        ]);
        let lines = ok("SID=a\r\nHSID=b\r\n");
        assert_eq!(lines.cookies.len(), 2);
    }

    #[test]
    fn text_that_is_no_shape_at_all_is_refused() {
        for text in ["hello world", "SID=a; and then some prose", "a\tb\tc"] {
            assert_eq!(parse(text).unwrap_err().to_string(), UNRECOGNISED, "{text:?}");
        }
        assert_eq!(parse("  \n ").unwrap_err().to_string(), "it is empty");
    }

    // -- the copy yt-dlp reads

    #[test]
    fn the_copy_round_trips_and_keeps_the_flag_and_the_dot_in_step() {
        let cookies = vec![
            c(".youtube.com", true, 1790000000, "SID", "a"),
            c("www.youtube.com", false, 0, "PREF", "b=c"),
        ];
        let text = to_netscape(&cookies);
        assert!(text.starts_with("# Netscape HTTP Cookie File\n"));
        assert!(text.contains(".youtube.com\tTRUE\t/\tTRUE\t1790000000\tSID\ta\n"));
        assert!(text.contains("www.youtube.com\tFALSE\t/\tFALSE\t0\tPREF\tb=c\n"));
        assert_eq!(ok(&text).cookies, cookies);
    }

    #[test]
    fn utf16_with_a_bom_decodes_and_a_browser_database_is_named() {
        let mut le = vec![0xFF, 0xFE];
        for u in "SID=a".encode_utf16() {
            le.extend(u.to_le_bytes());
        }
        assert_eq!(decode(&le).unwrap(), "SID=a");
        let e = decode(b"SQLite format 3\0\x10\0").unwrap_err().to_string();
        assert!(e.contains("browser's own cookie database"), "{e}");
        assert_eq!(decode(&[0xC3, 0x28]).unwrap_err().to_string(), "it is not a text file");
    }

    #[test]
    fn inspect_reports_the_shape_and_whether_youtube_is_signed_in() {
        let tmp = tempfile::tempdir().unwrap();
        let f = tmp.path().join("c.json");
        std::fs::write(&f, r#"[{"name": "LOGIN_INFO", "value": "x", "domain": ".youtube.com"},
                               {"name": "NID", "value": "y", "domain": ".google.com"}]"#).unwrap();
        assert_eq!(inspect(&f).unwrap(), CookiesFileInfo {
            format: "JSON cookie export".into(), cookies: 2, skipped: 0, youtube: true, signed_in: true,
        });
        std::fs::write(&f, r#"[{"name": "NID", "value": "y", "domain": ".google.com"}]"#).unwrap();
        let i = inspect(&f).unwrap();
        assert!(!i.youtube && !i.signed_in);
    }

    #[test]
    fn inspect_words_a_failure_for_the_person_who_chose_the_file() {
        let tmp = tempfile::tempdir().unwrap();
        let f = tmp.path().join("notes.txt");
        std::fs::write(&f, "remember the milk").unwrap();
        assert_eq!(
            inspect(&f).unwrap_err().to_string(),
            format!("notes.txt can't be used as a cookies file: {UNRECOGNISED}."),
        );
        let gone = tmp.path().join("gone.txt");
        assert_eq!(
            inspect(&gone).unwrap_err().to_string(),
            format!("The cookies file {} no longer exists.", gone.display()),
        );
    }

    #[test]
    fn prepare_writes_a_private_copy_and_never_touches_the_source() {
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("export.json");
        let original = r#"[{"name": "SID", "value": "a", "domain": ".youtube.com", "secure": true}]"#;
        std::fs::write(&src, original).unwrap();
        let store = tmp.path().join("store");

        let copy = prepare(&src, &store).unwrap();
        assert_eq!(copy.parent(), Some(store.as_path()));
        let text = std::fs::read_to_string(&copy).unwrap();
        assert!(text.contains(".youtube.com\tTRUE\t/\tTRUE\t0\tSID\ta\n"), "{text}");
        assert_eq!(std::fs::read_to_string(&src).unwrap(), original);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(&copy).unwrap().permissions().mode() & 0o777, 0o600);
            assert_eq!(std::fs::metadata(&store).unwrap().permissions().mode() & 0o777, 0o700);
        }
        // Nothing but the copy is left behind.
        assert_eq!(std::fs::read_dir(&store).unwrap().count(), 1);
    }

    #[test]
    fn prepare_keeps_what_yt_dlp_saved_until_the_source_changes() {
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("cookies.txt");
        let store = tmp.path().join("store");
        std::fs::write(&src, "SID=a").unwrap();
        let copy = prepare(&src, &store).unwrap();

        // yt-dlp rewrites the copy with a cookie YouTube refreshed.
        let refreshed = "# Netscape HTTP Cookie File\n.youtube.com\tTRUE\t/\tTRUE\t0\tSID\trotated\n";
        std::fs::write(&copy, refreshed).unwrap();
        assert_eq!(prepare(&src, &store).unwrap(), copy);
        assert_eq!(std::fs::read_to_string(&copy).unwrap(), refreshed);

        // A copy that no longer loads is rebuilt from the source.
        std::fs::write(&copy, "").unwrap();
        assert_eq!(prepare(&src, &store).unwrap(), copy);
        assert!(std::fs::read_to_string(&copy).unwrap().contains("\tSID\ta\n"));

        // A re-exported source is a new copy, and the old one is swept.
        std::fs::write(&src, "SID=b").unwrap();
        let fresh = prepare(&src, &store).unwrap();
        assert_ne!(fresh, copy);
        assert!(!copy.exists());
        assert!(std::fs::read_to_string(&fresh).unwrap().contains("\tSID\tb\n"));
    }

    #[test]
    fn prepare_fails_on_a_file_it_cannot_read() {
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("bad.txt");
        std::fs::write(&src, "remember the milk").unwrap();
        let e = prepare(&src, &tmp.path().join("store")).unwrap_err().to_string();
        assert!(e.starts_with("bad.txt can't be used as a cookies file"), "{e}");
    }
}
