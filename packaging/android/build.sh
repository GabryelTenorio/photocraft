#!/usr/bin/env bash
# PhotoCraft APK for Android tablets (arm64): needs the Android SDK + NDK, `rustup target add
# aarch64-linux-android` and `cargo install cargo-apk`. Output: target/android/release/apk/.
set -euo pipefail
cd "$(dirname "$0")/../.."
: "${ANDROID_HOME:=$HOME/Android/Sdk}"
: "${ANDROID_NDK_ROOT:=$(ls -d "$ANDROID_HOME"/ndk/* | sort -V | tail -1)}"
export ANDROID_HOME ANDROID_NDK_ROOT ANDROID_NDK_HOME="$ANDROID_NDK_ROOT"
# Sideload signing with the debug keystore; set both variables to sign with your own.
export CARGO_APK_RELEASE_KEYSTORE="${CARGO_APK_RELEASE_KEYSTORE:-$HOME/.android/debug.keystore}"
export CARGO_APK_RELEASE_KEYSTORE_PASSWORD="${CARGO_APK_RELEASE_KEYSTORE_PASSWORD:-android}"
CARGO_TARGET_DIR=target/android cargo apk build -p photocraft-android --lib --release
ls -l target/android/release/apk/*.apk
