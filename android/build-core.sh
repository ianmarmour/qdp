#!/usr/bin/env bash
# Builds the app's Rust core, libqdp_android.so, for 64-bit ARM, and puts it
# where the app packages it: android/app/src/main/jniLibs/arm64-v8a.
#
# It needs Rust's aarch64-linux-android target (rustup target add
# aarch64-linux-android) and the Android NDK, found at $ANDROID_NDK_HOME or
# as the newest under $ANDROID_HOME/ndk.
set -euo pipefail

# The oldest Android the app runs on: 10, whose MediaCodec decodes AV1 on
# devices with an AV1 decoder, and whose AAudio plays with low latency.
api=29
target=aarch64-linux-android
abi=arm64-v8a

root=$(cd "$(dirname "$0")/.." && pwd)
ndk=${ANDROID_NDK_HOME:-}
if [ -z "$ndk" ]; then
    ndk=$(ls -d "${ANDROID_HOME:?set ANDROID_HOME or ANDROID_NDK_HOME}"/ndk/* | sort -V | tail -n 1)
fi
# The NDK's tools for macOS are universal binaries in darwin-x86_64.
host=$(uname -s | tr '[:upper:]' '[:lower:]')-x86_64
toolchain=$ndk/toolchains/llvm/prebuilt/$host/bin
export CC_aarch64_linux_android=$toolchain/$target$api-clang
export CXX_aarch64_linux_android=$toolchain/$target$api-clang++
export AR_aarch64_linux_android=$toolchain/llvm-ar
export CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER=$toolchain/$target$api-clang

# The software decoder's assembly refers to tables the Rust code defines
# and exports; -Bsymbolic binds those references within the library, as a
# shared library's position-independent code needs.
cargo rustc --manifest-path "$root/Cargo.toml" -p qdp-android --lib \
    --target "$target" --release --crate-type cdylib \
    -- -C link-arg=-Wl,-Bsymbolic

mkdir -p "$root/android/app/src/main/jniLibs/$abi"
cp "$root/target/$target/release/libqdp_android.so" "$root/android/app/src/main/jniLibs/$abi/"
echo "built android/app/src/main/jniLibs/$abi/libqdp_android.so"
