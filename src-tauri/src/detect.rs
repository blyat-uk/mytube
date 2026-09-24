//! What is installed on this machine that MyTube could use: video players and
//! browsers whose cookies yt-dlp can read.
//!
//! FOUNDATION STUB: the detect task replaces every body.

use crate::config::Settings;
use crate::models::{BrowserOption, PlayerOption};
use crate::ytdlp::Cookies;

/// "System default" (`command: ""`) first, then installed players in this OS's
/// preference order.
pub fn detect_players() -> Vec<PlayerOption> {
    todo!("detect task")
}

/// Browsers whose profile data exists, in yt-dlp's own paths.
pub fn detect_browsers() -> Vec<BrowserOption> {
    todo!("detect task")
}

/// The cookie source a yt-dlp run should use for these settings:
/// `cookies_file` wins; `auto` is Firefox when a Firefox profile exists, else
/// none; `""` is none; anything else is a browser spec verbatim.
pub fn resolve_cookies(s: &Settings) -> Cookies {
    let _ = s;
    todo!("detect task")
}
