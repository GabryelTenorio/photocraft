//! The PhotoCraft shell, shared by the desktop and Android entries.
//!
//! Both `main.rs` (desktop `eframe::run_native`) and `lib.rs` (Android `android_main`) assemble the
//! same editor the same way, mirroring how `apps/photocraft-web` reuses
//! [`photocraft_ui_egui::PhotocraftApp`] with platform-specific [`Services`]: a fresh `Session`
//! plus the caller's `Services` become one `PhotocraftApp`. Each entry then wires its platform
//! extras onto the returned app.

use photocraft_engine::Session;
use photocraft_ui_egui::{PhotocraftApp, Services};

/// Build the PhotoCraft editor around the platform `Services` the caller prepared.
///
/// The desktop entry follows this with its desktop-only services (tablet, screen colour, macOS
/// menu, GPU startup, …); the Android entry leaves them at their `None`/`false` defaults and the
/// editor degrades gracefully (no file dialog, no clipboard, no native menu).
pub fn build_app(services: Services) -> PhotocraftApp {
    let mut app = PhotocraftApp::new(Session::new(), services);
    // Long commands and file opens run as background jobs with progress and Cancel (#210).
    app.background_jobs = std::env::var_os("PHOTOCRAFT_INLINE_JOBS").is_none();
    app
}
