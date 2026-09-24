//! System tray icon: close-to-tray's other half, the unread badge, and
//! click-to-toggle.
//!
//! One behaviour, two backends, chosen at compile time:
//!
//! - **Linux** ([`sni`]) speaks StatusNotifierItem itself through `ksni`,
//!   because Tauri's tray there is libayatana-appindicator, which never tells
//!   the application a left click happened. That module's doc has the detail.
//! - **macOS and Windows** ([`native`]) use Tauri's own tray, which does report
//!   clicks on those platforms, so the same left-click toggle comes for free.
//!
//! Everything the two share lives here: the eleven badge icons, the pure count
//! rules, and what "show" and "toggle" mean for the window. A backend supplies
//! only `init`, `is_available` and `update` -- the one place the pending count
//! is read and written -- so the badge behaves identically on every OS.

use tauri::AppHandle;

use crate::window;

#[cfg(target_os = "linux")]
mod sni;
#[cfg(target_os = "linux")]
use sni as backend;

#[cfg(not(target_os = "linux"))]
mod native;
#[cfg(not(target_os = "linux"))]
use native as backend;

pub const TRAY_ID: &str = "mytube";

/// Baked into the binary rather than read from disk: eleven small PNGs cost
/// ~40KB, and an icon the packaging step forgot to install would be a tray
/// that silently stops updating.
const ICONS: [&[u8]; 11] = [
    include_bytes!("../../icons/tray.png"),
    include_bytes!("../../icons/tray-1.png"),
    include_bytes!("../../icons/tray-2.png"),
    include_bytes!("../../icons/tray-3.png"),
    include_bytes!("../../icons/tray-4.png"),
    include_bytes!("../../icons/tray-5.png"),
    include_bytes!("../../icons/tray-6.png"),
    include_bytes!("../../icons/tray-7.png"),
    include_bytes!("../../icons/tray-8.png"),
    include_bytes!("../../icons/tray-9.png"),
    include_bytes!("../../icons/tray-9plus.png"),
];

/// Which icon a pending count should show: 0 is the plain icon, 1..=9 are the
/// numbered badges, and 10 is "9+".
pub fn badge_slot(count: u32) -> usize {
    count.min(10) as usize
}

/// The pending count after a poll that found `new` videos.
pub fn next_count(current: u32, new: usize, window_visible: bool) -> u32 {
    if window_visible {
        return 0;
    }
    current.saturating_add(new.min(u32::MAX as usize) as u32)
}

/// Registers the tray. An `Err` leaves the app running without one.
///
/// Returning means the item is really up (or really failed): the very next
/// thing the caller does is wire the window, whose close handler asks
/// [`is_available`] whether there is anywhere to close *to*.
pub fn init(app: &AppHandle) -> Result<(), String> {
    backend::init(app)
}

/// Whether a tray actually registered, and is still there.
///
/// Unset means there was nothing to register with, and [`window`] then treats
/// a close as a real close -- a tray that never appeared must not strand the
/// window off screen.
pub fn is_available() -> bool {
    backend::is_available()
}

/// Raises the window and clears the badge, which is what "read them" means.
pub fn show_window(app: &AppHandle) {
    let Some(win) = window::main_window(app) else {
        return;
    };
    let _ = win.unminimize();
    let _ = win.show();
    let _ = win.set_focus();
    clear();
}

/// Left click: put it away if you can see it, raise it if you cannot.
///
/// Hiding goes through `close()` rather than `hide()` so it takes the same
/// `CloseRequested` path the window's own close button does. That handler is
/// what records the geometry, and a tray toggle should not become the one way
/// of putting the window away that forgets where it was.
fn toggle_window(app: &AppHandle) {
    let Some(win) = window::main_window(app) else {
        return;
    };
    if window::is_showing(win.clone()) {
        let _ = win.close();
    } else {
        show_window(app);
    }
}

/// Adds a poll's haul to the badge, or does nothing if the window is on screen.
pub fn note_new_videos(app: &AppHandle, new: usize) {
    let showing = window::main_window(app).is_some_and(window::is_showing);
    backend::update(move |pending| *pending = next_count(*pending, new, showing));
}

/// Back to the plain icon, with nothing outstanding.
pub fn clear() {
    backend::update(|pending| *pending = 0);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nothing_pending_uses_the_plain_icon() {
        assert_eq!(badge_slot(0), 0);
    }

    #[test]
    fn a_single_digit_count_uses_its_own_badge() {
        assert_eq!(badge_slot(1), 1);
        assert_eq!(badge_slot(9), 9);
    }

    #[test]
    fn ten_and_beyond_share_the_nine_plus_badge() {
        assert_eq!(badge_slot(10), 10);
        assert_eq!(badge_slot(999), 10);
    }

    #[test]
    fn new_videos_accumulate_while_the_window_is_hidden() {
        assert_eq!(next_count(0, 3, false), 3);
        assert_eq!(next_count(3, 2, false), 5);
    }

    #[test]
    fn a_poll_that_found_nothing_leaves_the_count_alone() {
        assert_eq!(next_count(3, 0, false), 3);
    }

    #[test]
    fn a_visible_window_keeps_the_count_at_zero() {
        // The grid already refetches on `poll://finished`, so anything found
        // while you are looking at it has been read by definition.
        assert_eq!(next_count(0, 3, true), 0);
        assert_eq!(next_count(3, 2, true), 0);
    }

    #[test]
    fn every_badge_png_decodes_as_a_tauri_image() {
        // The native backend hands these straight to `set_icon` through
        // `Image::from_bytes`; checked here, on every OS, so a bad PNG fails a
        // Linux test run rather than a Windows or macOS tray at runtime.
        for (slot, png) in ICONS.iter().enumerate() {
            let img = tauri::image::Image::from_bytes(png)
                .unwrap_or_else(|e| panic!("icon {slot} failed to decode: {e}"));
            assert!(img.width() > 0 && img.height() > 0, "icon {slot} is empty");
        }
    }
}
