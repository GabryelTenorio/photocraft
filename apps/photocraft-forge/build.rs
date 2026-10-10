//! Capture the build's target triple and Cargo profile as compile-time env vars, so Forge can
//! report its own build identity (research §6/§14 Phase 0). Environment-independent: nothing here
//! reads user config or the network.

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    let target = std::env::var("TARGET").unwrap_or_default();
    println!("cargo:rustc-env=FORGE_BUILD_TARGET={target}");
    let profile = std::env::var("PROFILE").unwrap_or_default();
    println!("cargo:rustc-env=FORGE_BUILD_PROFILE={profile}");
}
