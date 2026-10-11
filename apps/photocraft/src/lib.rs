//! PhotoCraft Android entry point.
//!
//! This is the library target declared in `Cargo.toml` (`crate-type = ["cdylib", "rlib"]`). On
//! Android it compiles to `libphotocraft.so`, whose exported `android_main` is invoked by
//! `android.app.NativeActivity` through `android-activity`'s glue (the same backend winit 0.30 /
//! eframe 0.36 use). On every other platform this crate is empty; the desktop binary lives in
//! `main.rs`.

#![cfg(target_os = "android")]
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

mod app;

use android_activity::AndroidApp;

/// NativeActivity JNI entry point.
///
/// The signature matches `android-activity`'s `extern "Rust" { fn android_main(app: AndroidApp) }`
/// contract: it must be exported under the unmangled name `android_main` with the Rust ABI. eframe
/// takes the `AndroidApp` and builds the winit event loop with `with_android_app`.
///
/// `#[unsafe(no_mangle)]` is the one unsafe op this crate performs (there is no safe way to export
/// an unmangled symbol); the crate-level `unsafe_code = "deny"` plus this scoped allow keeps the
/// rest of the workspace `forbid`.
#[allow(unsafe_code)]
#[unsafe(no_mangle)]
fn android_main(app: AndroidApp) {
    // TODO(android): attach an `android_logger` sink so `log::*` output reaches `adb logcat`.

    // Android has no system title bar to replace, so the desktop window strategy (`CUSTOM_TITLEBAR`,
    // `with_decorations(!custom_titlebar)`, `integrated_titlebar` / `custom_titlebar`) is skipped:
    // the `PhotocraftApp` fields stay at their defaults (`false`) and the activity is fullscreen.
    // `min_inner_size` still bounds freeform (multi-window) resizing; see the Android manifest.
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("PhotoCraft")
            .with_inner_size([1440.0, 900.0])
            .with_min_inner_size([760.0, 480.0]),
        android_app: Some(app),
        ..Default::default()
    };

    // The same shell as the desktop: `app::build_app` assembles `Session + Services + PhotocraftApp`
    // (the pattern `apps/photocraft-web` reuses). Desktop services (file dialogs, clipboard, browser
    // opener, …) are absent here — `Services::default()` leaves them `None` — and the editor
    // degrades gracefully.
    let result = eframe::run_native(
        "PhotoCraft",
        options,
        Box::new(|_cc| Ok(Box::new(app::build_app(photocraft_ui_egui::Services::default())))),
    );
    if let Err(error) = result {
        log::error!("PhotoCraft Android failed to start: {error}");
    }
}
