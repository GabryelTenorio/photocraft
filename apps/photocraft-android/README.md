# photocraft-android

PhotoCraft on Android tablets with pen support. Native Rust: the same `PhotocraftApp` as the
desktop app, in an eframe NativeActivity on wgpu/Vulkan (no wasm, webview or JavaScript).
The shell is modelled on `photocraft-web` (thin `Services`, no desktop-only modules), not on its
technology.

## Build

Needs the Android SDK + NDK, `rustup target add aarch64-linux-android` and
`cargo install cargo-apk`.

```sh
packaging/android/build.sh      # -> target/android/release/apk/photocraft-android.apk
adb install -r target/android/release/apk/photocraft-android.apk
```

The APK is arm64 only, minSdk 29, signed with the Android debug keystore (sideload). Set
`CARGO_APK_RELEASE_KEYSTORE` and `CARGO_APK_RELEASE_KEYSTORE_PASSWORD` to sign with your own.

## What is in it

| Area | How |
|---|---|
| Entry point | `src/entry.rs`: `android_main`, the platform `Services` (import, export, prefs in the app's files dir), a `Shell` wrapping `PhotocraftApp`. The only `unsafe` is the `#[unsafe(no_mangle)]` entry (this crate denies `unsafe` instead of inheriting the workspace `forbid`, like `photocraft-tablet`). |
| Pen pressure | winit forwards Android pressure as `egui::Event::Touch { force }`; `ui-egui/src/stylus.rs` turns it into brush pressure. Tilt, twist and the eraser end are not exposed by winit 0.30. |
| Pen vs. finger | `patches/winit` (winit 0.30.13 + one change, see `patches/README.md`) puts the Android tool type in the high bits of `Touch::id`. `Stylus::finger_down()` is true while a finger is down and no pen is (palm rejection); `canvas.rs` then switches to the Hand tool, so fingers pan and pinch-zoom but never paint. Other platforms never set the flag bits, so nothing changes there. |
| File dialogs | `src/browser.rs`: an in-app browser over `/storage/emulated/0` (Open with multi-select and extension filter, Save As with a name field). Needs "All files access" (`MANAGE_EXTERNAL_STORAGE`); the dialog's button opens the settings page with `am start`. The Storage Access Framework would need a Java result callback, which a NativeActivity without Java glue can't receive. Not suitable for the Play Store. |
| System bars | `target_sdk_version = 34` (from 35 Android forces edge-to-edge), theme `NoActionBar.Fullscreen`, and the app is laid out inside the activity's `content_rect()`. |
| accesskit | `crates/ui-egui/Cargo.toml` enables eframe's `accesskit` only off Android (it needs `android-game-activity`; this build uses NativeActivity). |

## Not there yet

- Clipboard, autosave/recovery, the `--control` server, brush preset store, monitor profiles
  (desktop-only services).
- Pen tilt, rotation and eraser-end switching.
- Verified on a real tablet only for: it starts, pen pressure works. The system-bar fixes, the file
  browser and finger pan were built but not confirmed on a device.
- x86_64 / armv7 targets (add to `build_targets` in `Cargo.toml`).
