# Patched dependencies

`winit/` is winit 0.30.13 (Apache-2.0, license file inside) with one change in
`src/platform_impl/android/mod.rs`: the Android tool type (stylus / eraser / finger) is encoded in
the high bits of `Touch::id`, because winit 0.30 drops it and PhotoCraft Android must tell the pen
from fingers (`crates/ui-egui/src/stylus.rs`). Hooked up by `[patch.crates-io]` in the root
`Cargo.toml`. Drop it when winit exposes pointer kinds (0.31's `PointerKind`).
