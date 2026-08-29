// Prevents additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    #[cfg(target_os = "linux")]
    linux_webkit_workaround();
    mytube_lib::run()
}

/// WebKitGTK's DMA-BUF renderer makes GDK abort during initialisation on some
/// Wayland compositors, killing the app before a window ever appears:
///
///     Gdk-Message: Error 71 (Protocol error) dispatching to Wayland display.
///
/// Reproduced 3/3 on KDE Plasma (kwin_wayland) with webkit2gtk 2.52.6; disabling
/// the DMA-BUF renderer fixed it 3/3 while keeping the app on native Wayland,
/// which `GDK_BACKEND=x11` would have given up.
///
/// Set here rather than in the .desktop file so it applies however the app is
/// started, and only when the user has not already chosen a value. Must run
/// before Tauri initialises GTK.
#[cfg(target_os = "linux")]
fn linux_webkit_workaround() {
    if std::env::var_os("WEBKIT_DISABLE_DMABUF_RENDERER").is_none() {
        std::env::set_var("WEBKIT_DISABLE_DMABUF_RENDERER", "1");
    }
}
