#!/usr/bin/env bash
# Builds the app's Rust core, libqdp_android.so, for 64-bit ARM, and puts it
# where the app packages it: android/app/src/main/jniLibs/arm64-v8a.
#
# It needs Rust's aarch64-linux-android target (rustup target add
# aarch64-linux-android) and the Android NDK, found as ndk-env.sh says.
set -euo pipefail

abi=arm64-v8a
root=$(cd "$(dirname "$0")/.." && pwd)
# shellcheck source=ndk-env.sh
. "$root/android/ndk-env.sh"

# The software decoder's assembly refers to tables the Rust code defines
# and exports; -Bsymbolic binds those references within the library, as a
# shared library's position-independent code needs.
# Google Play takes only libraries whose segments are aligned to 16 KB pages,
# which NDKs before r28 do not do unless asked.
cargo rustc --manifest-path "$root/Cargo.toml" -p qdp-android --lib \
    --target "$QDP_ANDROID_TARGET" --release --crate-type cdylib \
    -- -C link-arg=-Wl,-Bsymbolic -C link-arg=-Wl,-z,max-page-size=16384

mkdir -p "$root/android/app/src/main/jniLibs/$abi"
cp "$root/target/$QDP_ANDROID_TARGET/release/libqdp_android.so" "$root/android/app/src/main/jniLibs/$abi/"
echo "built android/app/src/main/jniLibs/$abi/libqdp_android.so"
