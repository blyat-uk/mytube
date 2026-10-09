//! The Turnstile window: the one human check before a group is created.
//!
//! Creating a group is the only request that costs the server storage, so it
//! alone asks for proof of a person. The page is the sync server's own
//! `/verify`, opened in a window of its own; on success it navigates to
//! `/verify/done?token=…`, and the navigation handler here catches that,
//! cancels it -- so `/verify/done` never reaches the network -- and hands the
//! token back.
//!
//! The window gets no IPC. `capabilities/default.json` lists only `"main"`,
//! and a capability is what grants a webview the right to invoke anything, so
//! a remote page in a window labelled `sync-verify` can call no command at all.
//! Keep it that way: the token comes back through the navigation, never
//! through `invoke`.
//!
//! `window::track` attaches close-to-tray to the main window alone, so closing
//! this one closes it -- which is how the user says "never mind".

use std::time::Duration;

use anyhow::{bail, Context, Result};
use tauri::{AppHandle, Manager, Url, WebviewUrl, WebviewWindowBuilder, WindowEvent};

use super::client::VERIFICATION_FAILED;

pub const LABEL: &str = "sync-verify";

/// Long enough for a challenge that needs solving by hand; short enough that a
/// window forgotten behind others does not hold Turn on open all afternoon.
const TIMEOUT: Duration = Duration::from_secs(5 * 60);

pub const CANCELLED: &str = "Verification was cancelled.";

/// What a navigation inside the window means.
#[derive(Debug, PartialEq, Eq)]
pub enum Nav {
    /// Anything else: the page itself, Turnstile's iframes from
    /// challenges.cloudflare.com, its scripts. All let through.
    Pass,
    /// `/verify/done?token=…` on the sync server.
    Token(String),
    /// `/verify/done` with no token: the page gave up.
    Failed,
}

/// Sorts one navigation. Only the server's own host and port count, so a page
/// elsewhere that happens to use the path cannot end the verification.
pub fn classify(nav: &Url, base: &Url) -> Nav {
    let same_origin = nav.host_str() == base.host_str()
        && nav.port_or_known_default() == base.port_or_known_default();
    if !same_origin || nav.path() != "/verify/done" {
        return Nav::Pass;
    }
    match nav.query_pairs().find(|(k, _)| k == "token") {
        Some((_, t)) if !t.is_empty() => Nav::Token(t.into_owned()),
        _ => Nav::Failed,
    }
}

enum Outcome {
    Token(String),
    Failed,
    Closed,
}

/// Opens the window on `<base>/verify` and waits for a token.
///
/// The caller must not hold sync's request lock while this waits: a person is
/// solving a puzzle, and the clock has every right to keep exchanging
/// meanwhile.
pub async fn token(app: &AppHandle, base: &str) -> Result<String> {
    if let Some(open) = app.get_webview_window(LABEL) {
        let _ = open.unminimize();
        let _ = open.set_focus();
        bail!("A verification window is already open. Finish it there, or close it to start again.");
    }
    let base_url = Url::parse(base).with_context(|| format!("{base} is not a sync server address"))?;
    let page = Url::parse(&format!("{base}/verify"))?;

    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let nav_tx = tx.clone();
    let window = WebviewWindowBuilder::new(app, LABEL, WebviewUrl::External(page))
        .title("Verify \u{2014} MyTube")
        .inner_size(440.0, 620.0)
        .center()
        .on_navigation(move |url| match classify(url, &base_url) {
            Nav::Pass => true,
            // Cancelled here and closed by the waiting side below: closing a
            // window from inside its own navigation callback is asking for a
            // re-entrancy problem on at least one of three webviews.
            Nav::Token(t) => {
                let _ = nav_tx.send(Outcome::Token(t));
                false
            }
            Nav::Failed => {
                let _ = nav_tx.send(Outcome::Failed);
                false
            }
        })
        .build()
        .context("could not open the verification window")?;
    window.on_window_event(move |event| {
        if matches!(event, WindowEvent::Destroyed) {
            let _ = tx.send(Outcome::Closed);
        }
    });

    let outcome = tokio::time::timeout(TIMEOUT, rx.recv()).await;
    // Whatever happened, the window has done its job. Already gone (the user
    // closed it) is fine.
    let _ = window.close();
    match outcome {
        Ok(Some(Outcome::Token(t))) => Ok(t),
        Ok(Some(Outcome::Failed)) => bail!(VERIFICATION_FAILED),
        Ok(Some(Outcome::Closed) | None) => bail!(CANCELLED),
        Err(_) => bail!("Verification took too long. Try again."),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn u(s: &str) -> Url {
        Url::parse(s).unwrap()
    }

    #[test]
    fn the_done_page_on_the_server_hands_back_its_token() {
        let base = u("https://mytube-sync.blyat.uk");
        assert_eq!(
            classify(&u("https://mytube-sync.blyat.uk/verify/done?token=0.abc-DEF_1"), &base),
            Nav::Token("0.abc-DEF_1".into())
        );
        // Percent-encoding in the token is undone.
        assert_eq!(
            classify(&u("https://mytube-sync.blyat.uk/verify/done?x=1&token=a%2Bb"), &base),
            Nav::Token("a+b".into())
        );
    }

    #[test]
    fn a_done_page_without_a_token_is_a_failure() {
        let base = u("https://mytube-sync.blyat.uk");
        assert_eq!(classify(&u("https://mytube-sync.blyat.uk/verify/done"), &base), Nav::Failed);
        assert_eq!(classify(&u("https://mytube-sync.blyat.uk/verify/done?token="), &base), Nav::Failed);
    }

    #[test]
    fn everything_else_is_let_through() {
        let base = u("https://mytube-sync.blyat.uk");
        for pass in [
            "https://mytube-sync.blyat.uk/verify",
            "https://challenges.cloudflare.com/cdn-cgi/challenge-platform/h/b/turnstile/if/ov2",
            "https://challenges.cloudflare.com/verify/done?token=forged",
            "https://mytube-sync.blyat.uk:8443/verify/done?token=other-port",
            "https://mytube-sync.blyat.uk/verify/done/extra?token=x",
            "about:blank",
        ] {
            assert_eq!(classify(&u(pass), &base), Nav::Pass, "{pass}");
        }
    }

    #[test]
    fn a_local_dev_server_matches_on_its_port() {
        let base = u("http://127.0.0.1:8787");
        assert_eq!(
            classify(&u("http://127.0.0.1:8787/verify/done?token=XXXX.DUMMY.TOKEN.XXXX"), &base),
            Nav::Token("XXXX.DUMMY.TOKEN.XXXX".into())
        );
        assert_eq!(classify(&u("http://127.0.0.1:8788/verify/done?token=t"), &base), Nav::Pass);
    }
}
