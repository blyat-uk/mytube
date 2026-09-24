fn main() {
    // tauri-build embeds its Common-Controls v6 manifest into the app binary
    // only. Test executables link the same comctl32 v6-only imports
    // (TaskDialogIndirect) without it, so on Windows the loader binds comctl32
    // 5.82 and `cargo test` dies with STATUS_ENTRYPOINT_NOT_FOUND before a
    // single test runs (tauri-apps/tauri#13419). Embedding one manifest into
    // every artifact through the MSVC linker covers bins and tests alike.
    //
    // Keyed on the *target*, not `cfg(windows)` (the host), and on MSVC only:
    // the mingw target used for cross-checking from Linux has no such flags.
    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    let target_env = std::env::var("CARGO_CFG_TARGET_ENV").unwrap_or_default();
    let msvc = target_os == "windows" && target_env == "msvc";

    let attributes = if msvc {
        let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("windows-app-manifest.xml");
        println!("cargo:rerun-if-changed={}", manifest.display());
        println!("cargo:rustc-link-arg=/MANIFEST:EMBED");
        println!("cargo:rustc-link-arg=/MANIFESTINPUT:{}", manifest.display());
        tauri_build::Attributes::new()
            .windows_attributes(tauri_build::WindowsAttributes::new_without_app_manifest())
    } else {
        tauri_build::Attributes::new()
    };
    tauri_build::try_build(attributes).expect("failed to run tauri-build");
}
