//! photocraft-testkit
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

pub mod fonts;
#[cfg(not(target_arch = "wasm32"))]
pub mod perf;
