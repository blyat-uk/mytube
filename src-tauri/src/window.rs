//! The main window's lifecycle: its geometry between runs, and closing it to
//! the tray rather than exiting.
//!
//! Geometry is kept in `settings.json` beside `card_size` rather than in
//! `tauri-plugin-window-state`, whose file lives under Tauri's
//! `app_config_dir()` — `~/.config/uk.blyat.mytube`, the directory
//! `config::config_dir` deliberately avoids.
//!
//! Close-to-tray lives here rather than in [`crate::tray`] because both halves
//! read the same `CloseRequested` event: splitting them across two listeners
//! would leave the order of "save the geometry" and "veto the close" up to the
//! order the listeners happened to be registered in.

use crate::config::{self, Settings};
use crate::tray;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use tauri::{AppHandle, LogicalPosition, LogicalSize, Manager, WebviewWindow, WindowEvent};

/// Set by [`quit`] so the `CloseRequested` handler lets that one close through.
static QUITTING: AtomicBool = AtomicBool::new(false);

/// The window everything else means when it says "the window".
///
/// The fallback covers a build where the config's `"label": "main"` has gone
/// missing: a mislabelled window must not read as no window at all.
pub fn main_window(app: &AppHandle) -> Option<WebviewWindow> {
    app.get_webview_window("main")
        .or_else(|| app.webview_windows().into_values().next())
}

/// Whether the user can currently see the window.
///
/// Errors read as "showing", the conservative answer: a badge that never
/// appears is a smaller failure than one that never goes away.
pub fn is_showing(win: WebviewWindow) -> bool {
    win.is_visible().unwrap_or(true) && !win.is_minimized().unwrap_or(false)
}

/// Really exit, from the tray's Quit item.
///
/// Routed through `close()` rather than `AppHandle::exit` so the quit path and
/// the hide path run the same `CloseRequested` handler, and the geometry is
/// saved either way. Closing the only window then exits the app.
pub fn quit(app: &AppHandle) {
    QUITTING.store(true, Ordering::SeqCst);
    match main_window(app) {
        Some(win) => {
            let _ = win.close();
        }
        None => app.exit(0),
    }
}

/// A window's placement in **logical** pixels. Logical rather than physical so
/// moving the window to a display with a different scale factor restores the
/// same apparent size rather than one scaled by the ratio between them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Geometry {
    pub width: u32,
    pub height: u32,
    /// `None` until a position is known. Wayland never tells a client where it
    /// is, so under it this stays whatever an X11 session last recorded.
    pub x: Option<i32>,
    pub y: Option<i32>,
}

impl Geometry {
    pub fn from_settings(s: &Settings) -> Self {
        Self { width: s.window_width, height: s.window_height, x: s.window_x, y: s.window_y }
    }

    fn write_into(self, s: &mut Settings) {
        s.window_width = self.width;
        s.window_height = self.height;
        s.window_x = self.x;
        s.window_y = self.y;
    }
}

/// Puts the window back where it was, then shows it.
///
/// The window is declared `"visible": false` so this runs before anything is
/// drawn — otherwise every launch would flash at the default 1280x840 and then
/// snap to the remembered size.
pub fn restore(win: &WebviewWindow, s: &Settings) {
    let g = Geometry::from_settings(s);
    let _ = win.set_size(LogicalSize::new(g.width, g.height));
    if let (Some(x), Some(y)) = (g.x, g.y) {
        // Ignored under Wayland, where the compositor owns placement.
        let _ = win.set_position(LogicalPosition::new(x, y));
    }
    if s.window_maximized {
        let _ = win.maximize();
    }
    // Unconditional: a failure above must not leave the app with no window.
    let _ = win.show();
}

/// Follows the window and writes its geometry to `settings.json` on close.
pub fn track(win: &WebviewWindow, start: Geometry) {
    let last = Arc::new(Mutex::new(start));
    let win_for_events = win.clone();

    win.on_window_event(move |event| match event {
        WindowEvent::Resized(_) | WindowEvent::Moved(_) => {
            let mut slot = last.lock().unwrap();
            if let Some(next) = read(&win_for_events, *slot) {
                *slot = next;
            }
        }
        // Saving only on close keeps this off the resize path, which fires on
        // every frame of a drag. A kill -9 loses the last move; a normal quit
        // never does. Closing to the tray runs this too, so a session that
        // ends with a hide records the geometry just the same.
        WindowEvent::CloseRequested { api, .. } => {
            // Close-to-tray means this now fires twice in a session: once on
            // the hide, once on the way out. Only the first has anything to
            // record — nothing moves a hidden window, and a hidden one can
            // report itself unmaximized, so persisting again on quit would
            // overwrite a remembered maximized state with `false`. The window
            // is still on screen at this point on the hide path, so `visible`
            // separates the two.
            if win_for_events.is_visible().unwrap_or(true) {
                let geometry = *last.lock().unwrap();
                persist(geometry, win_for_events.is_maximized().unwrap_or(false));
            }

            // Only with somewhere to close *to*. If the tray failed to build,
            // this stays an ordinary close rather than hiding the window with
            // no way left to bring it back.
            let has_tray = tray::is_available();
            if has_tray && !QUITTING.load(Ordering::SeqCst) {
                api.prevent_close();
                let _ = win_for_events.hide();
            }
        }
        // Belt and braces beside the tray's own reset: however the window came
        // back — the menu item, the compositor — the badge has been read.
        WindowEvent::Focused(true) => {
            tray::clear();
            wake_decorations(&win_for_events);
        }
        _ => {}
    });
}

/// Wakes the titlebar buttons back up after the window has been mapped again.
///
/// tao 0.35 draws its own client-side decorations on Wayland — a protocol
/// trace shows `set_window_geometry(26, 23, 1280, 887)`, i.e. a shadow margin
/// and a 47px titlebar the client owns — and they stop responding to the
/// pointer once the toplevel has been unmapped and mapped again. This app does
/// that twice over: the window is created `"visible": false`, and every close
/// hides it to the tray. The input region still covers the titlebar, so it is
/// the decoration's own state that goes stale, and any real size change
/// revives it. That is why maximizing and restoring fixes it by hand.
///
/// Toggling `resizable` is the cheapest size-change that leaves the window
/// looking identical. Upstream: tauri-apps/tauri#11856, fixed by
/// tauri-apps/tao#1218. Delete this once a Tauri release ships tao >= 0.36 —
/// `tauri-runtime-wry` 2.11.4 still pins 0.35.
fn wake_decorations(win: &WebviewWindow) {
    let _ = win.set_resizable(false);
    let _ = win.set_resizable(true);
}

fn read(win: &WebviewWindow, prev: Geometry) -> Option<Geometry> {
    let scale = win.scale_factor().ok()?;
    let size = win.outer_size().ok()?.to_logical::<f64>(scale);
    let pos = win.outer_position().ok().map(|p| p.to_logical::<f64>(scale));
    next_geometry(
        prev,
        win.is_maximized().unwrap_or(false),
        (size.width.round() as u32, size.height.round() as u32),
        pos.map(|p| (p.x.round() as i32, p.y.round() as i32)),
    )
}

/// The pure half of [`read`], so the maximize and Wayland rules are testable
/// without a real window. `None` means "keep what we had".
fn next_geometry(
    prev: Geometry,
    maximized: bool,
    size: (u32, u32),
    pos: Option<(i32, i32)>,
) -> Option<Geometry> {
    // A maximized window's size is the screen's, not the size it unmaximizes
    // back to. Recording it would lose the one worth remembering.
    if maximized {
        return None;
    }
    Some(Geometry {
        width: size.0,
        height: size.1,
        // Falling back to `prev` keeps a position recorded under X11 from being
        // erased by a later Wayland session, which cannot report one.
        x: pos.map(|p| p.0).or(prev.x),
        y: pos.map(|p| p.1).or(prev.y),
    })
}

/// Re-reads the file first: the Settings view writes it whole, so only the
/// window keys may be replaced here.
fn persist(geometry: Geometry, maximized: bool) {
    let Ok(mut s) = config::load() else { return };
    geometry.write_into(&mut s);
    s.window_maximized = maximized;
    if let Err(err) = config::save(&s) {
        eprintln!("mytube: could not save window geometry: {err}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PREV: Geometry = Geometry { width: 1000, height: 700, x: Some(40), y: Some(60) };

    #[test]
    fn an_ordinary_resize_is_recorded() {
        let g = next_geometry(PREV, false, (1440, 900), Some((10, 20))).unwrap();
        assert_eq!(g, Geometry { width: 1440, height: 900, x: Some(10), y: Some(20) });
    }

    #[test]
    fn a_maximized_frame_is_ignored_so_the_restore_size_survives() {
        assert_eq!(next_geometry(PREV, true, (3840, 2160), Some((0, 0))), None);
    }

    #[test]
    fn wayland_reports_no_position_and_the_old_one_is_kept() {
        let g = next_geometry(PREV, false, (1440, 900), None).unwrap();
        assert_eq!(g.width, 1440);
        assert_eq!(g.height, 900);
        assert_eq!((g.x, g.y), (Some(40), Some(60)), "a known position was thrown away");
    }

    #[test]
    fn an_unknown_position_stays_unknown_until_one_is_reported() {
        let blank = Geometry { width: 1280, height: 840, x: None, y: None };
        let g = next_geometry(blank, false, (1280, 840), None).unwrap();
        assert_eq!((g.x, g.y), (None, None));
    }

    #[test]
    fn geometry_round_trips_through_settings() {
        let mut s = Settings::default();
        Geometry { width: 1600, height: 1000, x: Some(-5), y: Some(12) }.write_into(&mut s);
        assert_eq!(
            Geometry::from_settings(&s),
            Geometry { width: 1600, height: 1000, x: Some(-5), y: Some(12) }
        );
    }
}
