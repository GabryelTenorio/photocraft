//! PhotoCraft Android: native shell and host-tested document services.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]
#[cfg(target_os = "android")]
mod android;
#[cfg(target_os = "android")]
mod android_files;
#[cfg(any(target_os = "android", test))]
mod services;
