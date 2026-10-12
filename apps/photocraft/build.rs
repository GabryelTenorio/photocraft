//! Embeds the Remote Canvas page (`apps/photocraft-remote/dist`, built by
//! `cargo xtask remote-page`) and, on Windows, the app icon and version info (VERSIONINFO) into
//! `photocraft.exe`.
//!
//! The icon step does nothing on other targets. A missing resource compiler is a warning, so a
//! cross-compile from macOS or Linux still links, unless `PHOTOCRAFT_REQUIRE_WINRES=1` (set by the
//! release workflow) turns it into an error.

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    remote_assets();
    println!("cargo:rerun-if-changed=../../assets/app-icon/photocraft.ico");
    println!("cargo:rerun-if-env-changed=PHOTOCRAFT_REQUIRE_WINRES");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let mut res = winresource::WindowsResource::new();
    res.set_icon("../../assets/app-icon/photocraft.ico")
        .set("ProductName", "PhotoCraft")
        .set("FileDescription", "PhotoCraft image editor")
        .set("CompanyName", "Learning Machines LLC")
        .set("LegalCopyright", "Copyright (c) the PhotoCraft authors. MIT OR Apache-2.0.")
        .set("OriginalFilename", "photocraft.exe")
        .set("InternalName", "photocraft");
    if let Err(e) = res.compile() {
        if std::env::var_os("PHOTOCRAFT_REQUIRE_WINRES").is_some() {
            panic!("embedding Windows resources failed: {e}");
        }
        println!("cargo:warning=photocraft.exe built without icon/version resources: {e}");
    }
}

/// Write `$OUT_DIR/remote_assets.rs`: the Remote Canvas page's files, or a page that says how to
/// build them when `apps/photocraft-remote/dist` hasn't been built.
fn remote_assets() {
    use std::fmt::Write as _;
    let (Ok(out), Ok(manifest)) = (std::env::var("OUT_DIR"), std::env::var("CARGO_MANIFEST_DIR")) else { return };
    let dist = std::path::Path::new(&manifest).join("../photocraft-remote/dist");
    println!("cargo:rerun-if-changed={}", dist.display());
    let files = [
        ("/index.html", "text/html; charset=utf-8", "index.html"),
        ("/photocraft_remote.js", "text/javascript", "photocraft_remote.js"),
        ("/photocraft_remote_bg.wasm", "application/wasm", "photocraft_remote_bg.wasm"),
        ("/manifest.webmanifest", "application/manifest+json", "manifest.webmanifest"),
        ("/icon.png", "image/png", "icon.png"),
    ];
    let mut code = String::from("pub const ASSETS: &[(&str, &str, &[u8])] = &[\n");
    let built = dist.join("photocraft_remote_bg.wasm").is_file();
    if built {
        for (route, mime, file) in files {
            let path = dist.join(file);
            println!("cargo:rerun-if-changed={}", path.display());
            if path.is_file() {
                let _ = writeln!(code, "    ({route:?}, {mime:?}, include_bytes!({:?})),", path.display().to_string());
            }
        }
    } else {
        println!("cargo:warning=Remote Canvas page not built (run cargo xtask remote-page); serving a placeholder");
        let page = "<!doctype html><meta name=viewport content=width=device-width><body style=font-family:sans-serif>The Remote Canvas page wasn't built into this PhotoCraft. Run <code>cargo xtask remote-page</code>, then rebuild.";
        let _ = writeln!(code, "    (\"/index.html\", \"text/html; charset=utf-8\", {:?}.as_bytes()),", page);
    }
    code.push_str("];\n");
    let _ = std::fs::write(std::path::Path::new(&out).join("remote_assets.rs"), code);
}
