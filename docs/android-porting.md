# Porting the Android files and touch fixes

The 0.8.2 community build pins upstream commit `24930b31` (full identity is recorded
in the exported manifest). Android version: `0.8.2-android-alpha.4`, versionCode 5.
The previous alpha.3 release stays available on its 0.7.0 source branch.

## Boundaries to keep when upgrading

| Module | Responsibility | Integration seam |
|---|---|---|
| `apps/photocraft-android/src/android.rs` | NativeActivity bootstrap and incoming intents | creates the editor and calls `android_files::install` |
| `apps/photocraft-android/src/android_files.rs` | SAF picker, bounded streams, save capability map, provider errors | existing `Services.file_dialog` and `Services.write` |
| `apps/photocraft-android/src/services.rs` | Rust import/export adapter | existing `Services.import` and `Services.export` |
| `apps/photocraft-android/android/.../FileHelper.java` | Android system-picker and VIEW/SEND lifecycle | JNI methods used only by the Android adapter |
| `crates/ui-egui/src/touch_navigation.rs` | touch ownership, centroid zoom/pan, stroke suppression | one `navigate` call in the canvas; returns a tool-input suppression flag |
| `crates/ui-egui/src/touch_ui.rs` | optional touch tray and one-shot sampling | raw input, canvas tool event, bottom panel; serialized `UiState.touch` |

The safe SAF implementation is now separate from the two existing unsafe JNI handle
conversions in the bootstrap. No new unsafe code was introduced. Minimal Java OS glue
and the existing JNI exception still require upstream review under the Rust-only rules.
Neither Android UI nor document-provider URI logic belongs in the image engine.

Keep these small shared-UI hooks during a future merge:

- `lib.rs`: register touch modules, call `touch_ui::raw_input` and `touch_ui::bar`.
- `state.rs`: defaulted serialized `UiState.touch`.
- `canvas.rs`: call `touch_navigation::navigate` before wheel/tool input; keep its suppression
  flag around hand pan, tool presses and flick inertia. Call `touch_ui::sample_event` before
  tool dispatch. `zoom_about` is crate-visible for centroid anchoring.
- `workspace_ui.rs`: `window.panel.modifierKeys` accepts `touchControls`; optional desktop checkbox.
- `panels.rs`: layer row/thumbnail/mask uses `sticky_mods`, so touch Ctrl/Shift reaches existing behavior.
- `retouch_ui.rs`: source hints switch to the touch instructions when the tray is enabled.
- `crates/text/src/{fonts,cjk}.rs`: Android system font discovery; keep new upstream font APIs.
- workspace Cargo registration, Android dependencies and `xtask/src/layers.rs` exemption.

Do not replace an entire upstream canvas or translation catalog with an old fork copy.
In 0.8.2 the editor has newer spring tools, transform behavior, egui/wgpu and more languages;
all remain upstream-owned. The tray dispatches existing commands rather than duplicating them.

## Export a reviewable migration bundle

Run from a committed source tree (Python 3, standard library only):

```sh
python packaging/android/port.py export --base 24930b31 --out plan/android-port-0.8.2
```

The bundle has `01-platform.patch`, `02-touch.patch`, `03-docs-tools.patch`, a keyed
`catalogs.json` and `manifest.json` with exact source/base commits. It exports tracked
Git objects only, so APKs, private signing keys, logs and personal screenshots are excluded.
The first patch includes the Android wrapper binary, so use Git's binary patch support.

On a clean future upstream branch, review and apply each patch with `git apply --3way`.
Resolve API changes before continuing to the next patch. The third patch contains this
guide and the migration/build helpers. Translations deliberately stay out of the touch patch:

```sh
git apply --3way /path/to/bundle/01-platform.patch
git apply --3way /path/to/bundle/02-touch.patch
git apply --3way /path/to/bundle/03-docs-tools.patch
python packaging/android/port.py catalogs --bundle /path/to/bundle --target .
```

The catalog tool adds missing `(context, source)` pairs, retains newer upstream wording,
rejects malformed/duplicate rows, and is idempotent. A removed language is reported rather
than recreated silently. A newly added language needs translations for the touch labels;
the normal complete-catalog regression catches omissions. This is an assisted port,
not a promise that arbitrary future Rust API changes merge without review.

## Build and validate

On Windows set `ANDROID_HOME`, `ANDROID_NDK_HOME`, `JAVA_HOME`, and the four signing
variables documented in the [Android README](../apps/photocraft-android/README.md).
Keep the same signing key and increase versionCode for an installable update.

```powershell
./packaging/android/build.ps1
```

The build helper produces an ARM64 APK and checksum under `target/android-dist`.
`-Unsigned` explicitly builds an unsigned package for local signing; do not distribute
it as an installable signed update. No machine paths or signing credentials are embedded.

Regression gates after a port:

```sh
python -m unittest discover -s packaging/android -p test_port.py
cargo test -p photocraft-android --lib
cargo test -p photocraft-ui-egui --lib
cargo clippy -p photocraft-ui-egui -p photocraft-android --all-targets -- -D warnings
cargo ndk -t arm64-v8a clippy -p photocraft-android --all-targets -- -D warnings
cargo xtask layers
cargo xtask wasm
cargo xtask scorecard
```

The UI suite includes seven `touch_ui::tests` and three `touch_navigation` canvas tests.
Check the offscreen snapshot visually and repeat the synthetic device walkthrough:
system picker open → PSD Save As → edit → Save again → reopen from Files;
source tap → clone paint → Undo/Redo; color sampling and selection add/subtract.
Use synthetic fixtures only. Preserve documented device/GPU limitations and distinguish
old screenshots from proof of the newly built APK. If font/import/export adapters change,
run the corpus gate as required by AGENTS.md.
