//! PhotoCraft Remote Canvas: the page a tablet (an iPad with Apple Pencil) opens to draw in the
//! desktop app. The desktop app serves it with `--remote` (`apps/photocraft/src/remote_server.rs`,
//! `photocraft_ui_egui::remote`); build it with `scripts/build-remote.sh`, which writes `dist/`
//! for the desktop build to embed.
//!
//! The page shows the app's window (or only the canvas) and sends input back: the Pencil with
//! pressure and tilt, fingers for navigation (see [`gesture`]), and a small toolbar.

// Natively only the tests use the decoder and gestures (the page is wasm).
#![cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

mod cursor;
mod frame;
mod gesture;
mod smooth;

#[cfg(target_arch = "wasm32")]
mod web;

#[cfg(target_arch = "wasm32")]
fn main() {
    if let Err(e) = web::start() {
        web::log(&format!("Remote Canvas failed to start: {e:?}"));
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn main() {
    eprintln!("photocraft-remote runs in a tablet's browser: build it with scripts/build-remote.sh and start PhotoCraft with --remote");
}
