//! The Linux tray backend: a StatusNotifierItem this app owns.
//!
//! This speaks the StatusNotifierItem spec directly (`ksni`) rather than going
//! through Tauri's tray, which on Linux is libayatana-appindicator. That
//! library's entire signal list is `new-icon`, `new-attention-icon`,
//! `new-status`, `new-label`, `connection-changed`, `new-icon-theme-path` and
//! `scroll-event`: it answers the spec's `Activate` itself by opening the menu
//! and never tells the application a click happened. Under it a left click
//! could only ever raise the menu, which is exactly what it used to do here.
//! Owning the item is what makes a left click ours to interpret. Don't go back
//! to `TrayIconBuilder` on Linux expecting clicks -- which is also why Tauri's
//! `tray-icon` feature is enabled only for the other targets.
//!
//! Right click still opens the menu -- the host builds it from [`Tray::menu`].

use std::sync::OnceLock;

use ksni::menu::StandardItem;
use ksni::{Handle, Icon, MenuItem, Tray, TrayMethods};
use tauri::AppHandle;

use super::{badge_slot, show_window, toggle_window, ICONS, TRAY_ID};
use crate::window;

/// The registered item, once it exists.
///
/// Unset means no StatusNotifierItem host was there to register with, and
/// [`window`] then treats a close as a real close -- a tray that never
/// appeared must not strand the window off screen.
static TRAY: OnceLock<Handle<MyTube>> = OnceLock::new();

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

pub(super) fn init(app: &AppHandle) -> Result<(), String> {
    let tray = MyTube { app: app.clone(), pending: 0 };
    // Blocking so that `init` returning means the item is really up: the very
    // next thing the caller does is wire the window, whose close handler asks
    // [`is_available`] whether there is anywhere to close *to*.
    let handle = tauri::async_runtime::block_on(tray.spawn()).map_err(|e| e.to_string())?;
    let _ = TRAY.set(handle);
    Ok(())
}

pub(super) fn is_available() -> bool {
    TRAY.get().is_some_and(|h| !h.is_closed())
}

/// Fire-and-forget, for two reasons. The callers arrive from three different
/// places -- the poll task, already async; the window event handler, on the
/// main thread; and ksni's own service thread -- and `Handle::update` takes
/// the service lock, which that third caller is already holding. Waiting for
/// it there would deadlock; handing it to the runtime lets the menu callback
/// return and release the lock first.
pub(super) fn update(f: impl FnOnce(&mut u32) + Send + 'static) {
    let Some(handle) = TRAY.get() else {
        return;
    };
    tauri::async_runtime::spawn(async move {
        handle.update(move |t: &mut MyTube| f(&mut t.pending)).await;
    });
}

#[cfg(test)]
mod tests {
    use super::*;

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
