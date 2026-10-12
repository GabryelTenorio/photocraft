//! `cargo xtask remote-page`: build the Remote Canvas page (`apps/photocraft-remote`) into
//! `apps/photocraft-remote/dist`, which the desktop app embeds at its next build. Needs the
//! `wasm32-unknown-unknown` target and `wasm-bindgen-cli` at the version in Cargo.lock; runs
//! `wasm-opt -Oz` too when it is installed.

use std::path::Path;
use std::process::Command;

use crate::{cargo, run};

pub fn run_cmd(root: &Path) -> Result<(), String> {
    let app = root.join("apps/photocraft-remote");
    let dist = app.join("dist");
    let tmp = app.join("dist.tmp");
    let target = std::env::var_os("CARGO_TARGET_DIR").map_or_else(|| root.join("target"), Into::into);
    let mut build = cargo();
    build.args(["build", "-p", "photocraft-remote", "--target", "wasm32-unknown-unknown", "--profile", "wasm-release"]);
    run(build, "cargo build -p photocraft-remote --target wasm32-unknown-unknown --profile wasm-release")?;
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).map_err(|e| format!("{}: {e}", tmp.display()))?;
    let wasm = target.join("wasm32-unknown-unknown/wasm-release/photocraft_remote.wasm");
    let mut bindgen = Command::new("wasm-bindgen");
    bindgen.args(["--target", "web", "--no-typescript", "--out-dir"]).arg(&tmp).arg(&wasm);
    run(bindgen, "wasm-bindgen (install: cargo install wasm-bindgen-cli --version <wasm-bindgen in Cargo.lock> --locked)")?;
    let bg = tmp.join("photocraft_remote_bg.wasm");
    if Command::new("wasm-opt").arg("--version").output().is_ok_and(|o| o.status.success()) {
        let mut opt = Command::new("wasm-opt");
        opt.arg("-Oz").arg("-o").arg(&bg).arg(&bg);
        run(opt, "wasm-opt -Oz")?;
    }
    let copy = |from: &Path, to: &str| std::fs::copy(from, tmp.join(to)).map(drop).map_err(|e| format!("{}: {e}", from.display()));
    copy(&app.join("static/index.html"), "index.html")?;
    copy(&app.join("static/manifest.webmanifest"), "manifest.webmanifest")?;
    copy(&root.join("assets/app-icon/photocraft-1024.png"), "icon.png")?;
    let _ = std::fs::remove_dir_all(&dist);
    std::fs::rename(&tmp, &dist).map_err(|e| format!("{}: {e}", dist.display()))?;
    eprintln!("Remote Canvas page built in {}; rebuild photocraft to embed it.", dist.display());
    Ok(())
}
