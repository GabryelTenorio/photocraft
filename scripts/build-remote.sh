#!/usr/bin/env bash
# Build the Remote Canvas page (apps/photocraft-remote) into apps/photocraft-remote/dist, which
# the desktop app embeds at its next build. Needs the wasm32 target and wasm-bindgen-cli at the
# version in Cargo.lock:
#   rustup target add wasm32-unknown-unknown
#   cargo install wasm-bindgen-cli --version <wasm-bindgen version in Cargo.lock> --locked
set -euo pipefail
root="$(cd "$(dirname "$0")/.." && pwd)"
app="$root/apps/photocraft-remote"
dist="$app/dist"
target="${CARGO_TARGET_DIR:-$root/target}"
cargo build --manifest-path "$root/Cargo.toml" -p photocraft-remote --target wasm32-unknown-unknown --profile wasm-release
rm -rf "$dist.tmp"
mkdir -p "$dist.tmp"
wasm-bindgen --target web --no-typescript --out-dir "$dist.tmp" "$target/wasm32-unknown-unknown/wasm-release/photocraft_remote.wasm"
if command -v wasm-opt >/dev/null; then
  wasm-opt -Oz -o "$dist.tmp/photocraft_remote_bg.wasm" "$dist.tmp/photocraft_remote_bg.wasm"
fi
cp "$app/static/index.html" "$app/static/manifest.webmanifest" "$dist.tmp/"
cp "$root/assets/app-icon/photocraft-1024.png" "$dist.tmp/icon.png"
rm -rf "$dist"
mv "$dist.tmp" "$dist"
echo "Remote Canvas page built in $dist ($(du -sh "$dist" | cut -f1)); rebuild photocraft to embed it."
