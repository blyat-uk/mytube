//! The macOS and Windows tray backend: Tauri's own tray (`tray-icon`).
//!
//! Unlike libayatana on Linux, `tray-icon` reports clicks on these two
//! platforms, so the left-click toggle needs nothing of our own:
//! `show_menu_on_left_click(false)` keeps the menu on the right button, and a
//! left button *release* toggles the window. Both platforms send a `Down` and
//! an `Up` for every click; acting on one of them is what keeps a single click
//! from toggling twice.
//!
//! The badge is the same eleven PNGs the Linux item shows, swapped in with
//! `set_icon`. Nothing here diffs the icon for us the way ksni does, so the
//! slot on screen is remembered beside the count and the icon is only replaced
//! when the slot actually moves -- a poll that takes 12 to 13 is a no-op.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};

use tauri::image::Image;
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::AppHandle;

use super::{badge_slot, show_window, toggle_window, ICONS, TRAY_ID};
use crate::window;

/// Menu ids. The builder's menu handler is global -- it also sees the macOS
/// app menu's items -- so these are matched exactly and anything else ignored.
const MENU_OPEN: &str = "mytube-tray-open";
const MENU_QUIT: &str = "mytube-tray-quit";

/// Set once the tray is built. The handle is what later badge updates go
/// through; the icon itself is looked up by [`TRAY_ID`] each time rather than
/// kept here, because Tauri drops its own reference at exit and a copy parked
/// in a static would outlive that -- on Windows an icon that is never dropped
/// is never removed, and lingers in the notification area after the app has
/// gone until the pointer happens to pass over it.
static APP: OnceLock<AppHandle> = OnceLock::new();
static BUILT: AtomicBool = AtomicBool::new(false);

struct Badge {
    /// Videos ingested since the window was last on screen.
    pending: u32,
    /// The slot currently showing in the tray.
    shown: usize,
}

static BADGE: Mutex<Badge> = Mutex::new(Badge { pending: 0, shown: 0 });

fn badge() -> std::sync::MutexGuard<'static, Badge> {
    // A panic while holding this cannot leave a count worse than stale.
    BADGE.lock().unwrap_or_else(|e| e.into_inner())
}

pub(super) fn init(app: &AppHandle) -> Result<(), String> {
    build(app).map_err(|e| e.to_string())?;
    let _ = APP.set(app.clone());
    BUILT.store(true, Ordering::SeqCst);
    Ok(())
}

fn build(app: &AppHandle) -> tauri::Result<()> {
    let open = MenuItem::with_id(app, MENU_OPEN, "Open MyTube", true, None::<&str>)?;
    let separator = PredefinedMenuItem::separator(app)?;
    let quit = MenuItem::with_id(app, MENU_QUIT, "Quit", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&open, &separator, &quit])?;

    // `build` runs on the main thread (inline when called from `setup`, which
    // is already there), so returning means the icon is really in the tray.
    TrayIconBuilder::with_id(TRAY_ID)
        .icon(Image::from_bytes(ICONS[0])?)
        .tooltip("MyTube")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| {
            let id = event.id().as_ref();
            if id == MENU_OPEN {
                show_window(app);
            } else if id == MENU_QUIT {
                window::quit(app);
            }
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                toggle_window(tray.app_handle());
            }
        })
        .build(app)?;
    Ok(())
}

pub(super) fn is_available() -> bool {
    BUILT.load(Ordering::SeqCst)
        && APP.get().is_some_and(|app| app.tray_by_id(TRAY_ID).is_some())
}

/// Changes the count, and schedules an icon swap if its slot moved.
///
/// Callers arrive from the poll task and from the main thread. The swap is
/// always handed to the main thread (inline if already there), and it reads
/// the count *when it runs* rather than carrying the value it was scheduled
/// with: two updates racing from two threads then cannot land their icons in
/// the wrong order and leave a badge that disagrees with the count.
pub(super) fn update(f: impl FnOnce(&mut u32) + Send + 'static) {
    let Some(app) = APP.get() else {
        return;
    };
    let moved = {
        let mut b = badge();
        f(&mut b.pending);
        badge_slot(b.pending) != b.shown
    };
    if moved {
        let handle = app.clone();
        let _ = app.run_on_main_thread(move || refresh(&handle));
    }
}

fn refresh(app: &AppHandle) {
    let mut b = badge();
    let slot = badge_slot(b.pending);
    if slot == b.shown {
        return;
    }
    // Gone at exit, once Tauri has let go of it: nothing left to badge.
    let Some(tray) = app.tray_by_id(TRAY_ID) else {
        return;
    };
    let Ok(icon) = Image::from_bytes(ICONS[slot]) else {
        return;
    };
    if tray.set_icon(Some(icon)).is_ok() {
        b.shown = slot;
    }
}
