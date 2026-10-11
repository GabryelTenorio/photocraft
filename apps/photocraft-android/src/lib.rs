//! PhotoCraft on Android tablets: the same [`photocraft_ui_egui::PhotocraftApp`] as the desktop
//! app, in an eframe NativeActivity (wgpu on Vulkan).
//!
//! Pen: winit forwards Android stylus/finger pressure as `egui::Event::Touch { force }`, which
//! the app's stylus module already turns into brush pressure (as on Windows). Tilt, twist and the
//! eraser end are not exposed by winit 0.30 yet.
//!
//! Files: File › Open / Save use an in-app browser over shared storage (`browser`), which needs
//! the "All files access" permission (the dialog offers to open its settings page).
//! Build: `packaging/android/build.sh`.

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

#[cfg(target_os = "android")]
mod browser;
#[cfg(target_os = "android")]
mod entry;
