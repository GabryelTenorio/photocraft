# Android alpha.2 validation — 2026-10-12

Base: PhotoCraft 0.7.0, commit 614d0689. Community build by @HubWithTorch.
APK: photocraft-android-v0.7.0-alpha.2.apk, ARM64, 58,726,588 bytes.
SHA256: 5BFB11ABC6389FAAA4C3788669DF7C9CF24E1FEC4B248F4FE3B8700D2253777D.

## Fixes

The initial local Android shell used independent uninitialized OnceLocks for picker
results, ended polling immediately, omitted import/export services and consumed a
single global save URI. It could neither reliably open nor repeatedly save documents.
The replacement uses a JVM global reference and Activity instance methods, starts
pickers on the Android UI thread, polls until selection/cancellation, reports errors,
closes streams, and retains per-target save capabilities. VIEW/SEND lifecycle events
are forwarded into the shared editor. The shell forwards eframe ui/logic/raw_input_hook.

Canvas touch navigation supports centroid zoom/pan and prevents stray brush strokes
when a second finger arrives or one finger remains. Three regression tests exercise
DPI/rotation/mirror/aspect, invalid input and an egui multi-touch event sequence. A host
service test exercises picker import, PNG export and two saves to the same target.

## Evidence

Lenovo TB378FC, Android 16, USB debugging: system picker PNG open, PSD Save As,
brush edit followed by Save, and Files cold launch of the saved PSD all succeeded.
The PSD grew from 46,494 to 50,501 bytes and reopened with the brush edit preserved.
Physical two-finger gesture feel is still awaiting user confirmation; synthetic touch
regressions are not a substitute for real multi-touch validation.

On the 0.7.0 base: Android service test (1), touch tests (3), UI unit tests (1953,
10 ignored), host and Android clippy, layers, wasm and xtask tests (48) passed.
An offscreen safe-GPU UI snapshot was visually inspected. Twelve host GPU integration
test targets failed with Microsoft Basic Render Driver; these are not claimed to pass.
Quick performance suite completed; some complex layout actions exceed 16 ms, and no
baseline was updated. These results apply to the tested 0.7.0 build, not future rebases.

## Source map

- src/android.rs: JNI bridge, document-provider streams, NativeActivity and intent loop.
- src/services.rs: import/export service wiring and host regression test.
- android/app/src/main/java/ai/storyteller/photocraft/FileHelper.java: system picker glue.
- crates/ui-egui/src/canvas.rs: shared touch navigation and tests.
- crates/text/src/{fonts,cjk}.rs: Android system font lookup.

The two existing unsafe JNI bootstrap calls are an explicit upstream-review item.
The original development notes are preserved locally in log/android-development-history.md.

## Upstream PR branch (0.8.0 base 4010630c)

The PR is forward-ported separately from the release APK. Android clippy (all targets),
3 touch tests, 1 service test, 134 text tests (4 ignored), 48 xtask tests, host clippy,
layers (30 crates), wasm and Gradle configuration passed. The safe-GPU snapshot was
visually checked. UI units: 2301 passed, 11 ignored, 1 failed in the unchanged upstream
`file_dialog::tests::export_as_with_several_artboards_selected_writes_one_file_each`:
Windows builds `/out\boards-2-Capsule.jpg`, whereas the assertion requires `/out/boards-2-`.
Complete corpus regression passed: 3153 tests passed, 25 ignored, across 143 test binaries. The local regression build used CARGO_PROFILE_RELEASE_LTO=false and CARGO_PROFILE_RELEASE_CODEGEN_UNITS=16; source/build defaults and the released APK are unchanged. The APK remains the previously tested 0.7.0 build.
The new Folder picker variant explicitly reports Android folder workflows unsupported.
Flick inertia and press-stroke state are suppressed during touch navigation.

## Alpha.3 touch tools (2026-10-13)

One-shot Clone/Healing source sampling consumes the complete pointer gesture, including
a duplicate Down on click release, so sampling never starts an accidental zero-offset
stroke. The bottom tray exposes color sampling, history, tool browsing, brush size,
selection modes and existing sticky modifiers. Layer rows now honor those modifiers.

On the 0.7.0 release source: 1960 UI tests passed (10 ignored), including seven new
touch tests; three pinch tests, host/cross clippy, layers and wasm passed. The release
APK was installed on Lenovo TB378FC / Android 16. ADB verified clone, Undo/Redo,
color sampling, brush size and selection add/subtract/deselect. The quick layout
benchmark completed; no baseline was updated and tablet frame time was not measured.
Crop/transform completion, polygon closure, fullscreen recovery and physical pinch
feel still require device confirmation. [Screenshots and reproduction steps](../../docs/android-touch-controls.md).

### Updated 0.8.0 PR validation

With the touch tray forward-port: 2308 UI units passed, 11 ignored, and the same
unchanged Windows export-artboard path separator assertion failed. All seven new
touch-control tests passed, including real canvas clicks and virtual Ctrl layer
selection. Host and Android clippy (all targets), layers and wasm passed. Vietnamese
labels were added without dropping newer upstream translations. The 0.8.0 press-stroke
boolean is cleared during sampling; Android development versionCode is 4.
The published APK remains the independently tested 0.7.0 alpha.3 build.

## 0.8.2 migration (alpha.4, 2026-10-13)

Pinned upstream: 24930b31 (0.8.2), preserving the original Android application id.
Android versionCode 5; source branch codex/android-0.8.2. Safe SAF code and service
registration moved into android_files.rs; the existing two unsafe bootstrap calls
remain in android.rs. Generic touch gesture ownership and navigation moved into
touch_navigation.rs. Source/color sampling remains in touch_ui.rs.

UI regression: 2545 passed, 12 ignored. Android host file service test passed.
The old Windows path assertion now passes on this upstream base. Host/Android clippy,
layers and wasm passed; safe-GPU synthetic snapshot inspected. Three new catalog-port
tests passed. The helper exports platform, touch and docs/tools patches plus keyed
catalog additions, preserving upstream translations. PSD export options added upstream
are forwarded by the Android service adapter. Final APK/device and corpus results
are recorded below once completed. [Porting guide](../../docs/android-porting.md).
