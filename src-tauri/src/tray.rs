//! System tray icon: close-to-tray's other half, the unread badge, and
//! click-to-toggle.
//!
//! This speaks the StatusNotifierItem spec directly (`ksni`) rather than going
//! through Tauri's tray, which on Linux is libayatana-appindicator. That
//! library's entire signal list is `new-icon`, `new-attention-icon`,
//! `new-status`, `new-label`, `connection-changed`, `new-icon-theme-path` and
//! `scroll-event`: it answers the spec's `Activate` itself by opening the menu
//! and never tells the application a click happened. Under it a left click
//! could only ever raise the menu, which is exactly what it used to do here.
//! Owning the item is what makes a left click ours to interpret.
//!
//! Right click still opens the menu -- the host builds it from [`Tray::menu`].

use std::sync::OnceLock;

use ksni::menu::StandardItem;
use ksni::{Handle, Icon, MenuItem, Tray, TrayMethods};
use tauri::AppHandle;

use crate::window;

pub const TRAY_ID: &str = "mytube";

/// Baked into the binary rather than read from disk: eleven small PNGs cost
/// ~40KB, and an icon the packaging step forgot to install would be a tray
/// that silently stops updating.
const ICONS: [&[u8]; 11] = [
    include_bytes!("../icons/tray.png"),
    include_bytes!("../icons/tray-1.png"),
    include_bytes!("../icons/tray-2.png"),
    include_bytes!("../icons/tray-3.png"),
    include_bytes!("../icons/tray-4.png"),
    include_bytes!("../icons/tray-5.png"),
    include_bytes!("../icons/tray-6.png"),
    include_bytes!("../icons/tray-7.png"),
    include_bytes!("../icons/tray-8.png"),
    include_bytes!("../icons/tray-9.png"),
    include_bytes!("../icons/tray-9plus.png"),
];

/// The registered item, once it exists.
///
/// Unset means no StatusNotifierItem host was there to register with, and
/// [`window`] then treats a close as a real close -- a tray that never
/// appeared must not strand the window off screen.
static TRAY: OnceLock<Handle<MyTube>> = OnceLock::new();

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

/// PNG decodes to RGBA; the spec wants ARGB32 in network byte order.
fn rgba_to_argb(mut data: Vec<u8>) -> Vec<u8> {
    for pixel in data.chunks_exact_mut(4) {
        pixel.rotate_right(1);
    }
    data
}

fn decode_icon(png: &[u8]) -> Option<Icon> {
    let img = tauri::image::Image::from_bytes(png).ok()?;
    Some(Icon {
        width: img.width() as i32,
        height: img.height() as i32,
        data: rgba_to_argb(img.rgba().to_vec()),
    })
}

/// Decoded once and kept: `icon_pixmap` is called on every property read, and
/// re-decoding eleven PNGs each time would be pure waste. A slot that fails to
/// decode yields no pixmap rather than a panic, which leaves the host free to
/// fall back on the themed icon name.
fn icon_for(count: u32) -> Vec<Icon> {
    static DECODED: OnceLock<Vec<Option<Icon>>> = OnceLock::new();
    let decoded = DECODED.get_or_init(|| ICONS.iter().map(|png| decode_icon(png)).collect());
    decoded[badge_slot(count)].clone().into_iter().collect()
}

struct MyTube {
    app: AppHandle,
    /// Videos ingested since the window was last on screen.
    ///
    /// State of the item itself, so ksni can diff it: it only emits `NewIcon`
    /// when the pixmap actually differs, which is why nothing here has to
    /// dedupe a poll that moves 12 to 13 by hand.
    pending: u32,
}

impl Tray for MyTube {
    fn id(&self) -> String {
        TRAY_ID.into()
    }

    fn title(&self) -> String {
        "MyTube".into()
    }

    fn icon_pixmap(&self) -> Vec<Icon> {
        icon_for(self.pending)
    }

    /// Left click -- the whole reason this module owns the item.
    fn activate(&mut self, _x: i32, _y: i32) {
        toggle_window(&self.app);
    }

    fn menu(&self) -> Vec<MenuItem<Self>> {
        vec![
            StandardItem {
                label: "Open MyTube".into(),
                activate: Box::new(|t: &mut Self| show_window(&t.app)),
                ..Default::default()
            }
            .into(),
            MenuItem::Separator,
            StandardItem {
                label: "Quit".into(),
                activate: Box::new(|t: &mut Self| window::quit(&t.app)),
                ..Default::default()
            }
            .into(),
        ]
    }
}

/// Registers the tray. An `Err` leaves the app running without one.
pub fn init(app: &AppHandle) -> Result<(), ksni::Error> {
    let tray = MyTube { app: app.clone(), pending: 0 };
    // Blocking so that `init` returning means the item is really up: the very
    // next thing the caller does is wire the window, whose close handler asks
    // [`is_available`] whether there is anywhere to close *to*.
    let handle = tauri::async_runtime::block_on(tray.spawn())?;
    let _ = TRAY.set(handle);
    Ok(())
}

/// Whether a tray actually registered, and is still there.
pub fn is_available() -> bool {
    TRAY.get().is_some_and(|h| !h.is_closed())
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
    update(move |t| t.pending = next_count(t.pending, new, showing));
}

/// Back to the plain icon, with nothing outstanding.
pub fn clear() {
    update(|t| t.pending = 0);
}

/// Fire-and-forget, for two reasons. The callers arrive from three different
/// places -- the poll task, already async; the window event handler, on the
/// main thread; and ksni's own service thread -- and `Handle::update` takes
/// the service lock, which that third caller is already holding. Waiting for
/// it there would deadlock; handing it to the runtime lets the menu callback
/// return and release the lock first.
fn update(f: impl FnOnce(&mut MyTube) + Send + 'static) {
    let Some(handle) = TRAY.get() else {
        return;
    };
    tauri::async_runtime::spawn(async move {
        handle.update(f).await;
    });
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
    fn rgba_to_argb_moves_alpha_to_the_front() {
        // The spec asks for ARGB32 in network byte order; PNG decodes to RGBA.
        assert_eq!(rgba_to_argb(vec![1, 2, 3, 4]), vec![4, 1, 2, 3]);
    }

    #[test]
    fn rgba_to_argb_converts_every_pixel() {
        assert_eq!(
            rgba_to_argb(vec![1, 2, 3, 4, 5, 6, 7, 8]),
            vec![4, 1, 2, 3, 8, 5, 6, 7]
        );
    }

    #[test]
    fn every_badge_icon_decodes_at_the_expected_size() {
        // A badge that fails to decode would be a tray that silently stops
        // updating, so all eleven are checked rather than just the plain one.
        for (slot, png) in ICONS.iter().enumerate() {
            let icon = decode_icon(png).unwrap_or_else(|| panic!("icon {slot} failed to decode"));
            assert_eq!(
                icon.data.len(),
                (icon.width * icon.height * 4) as usize,
                "icon {slot} is not four bytes per pixel"
            );
            assert!(icon.width > 0 && icon.height > 0, "icon {slot} is empty");
        }
    }

    #[test]
    fn every_badge_slot_has_an_icon_to_show() {
        for count in [0, 1, 5, 9, 10, 500] {
            assert_eq!(icon_for(count).len(), 1, "no pixmap for a count of {count}");
        }
    }
}
