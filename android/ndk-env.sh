# Sets up the Android NDK's compilers for cargo to build for 64-bit ARM
# Android: source it before building or checking the app's Rust core. The NDK
# is found at $ANDROID_NDK_HOME, or as the newest under $ANDROID_HOME/ndk.

# The oldest Android the app runs on: 10, whose MediaCodec decodes AV1 on
# devices with an AV1 decoder, and whose AAudio plays with low latency.
QDP_ANDROID_API=29
QDP_ANDROID_TARGET=aarch64-linux-android

qdp_ndk=${ANDROID_NDK_HOME:-}
if [ -z "$qdp_ndk" ]; then
    qdp_ndk=$(ls -d "${ANDROID_HOME:?set ANDROID_HOME or ANDROID_NDK_HOME}"/ndk/* | sort -V | tail -n 1)
fi
# The NDK's tools for macOS are universal binaries in darwin-x86_64.
qdp_toolchain=$qdp_ndk/toolchains/llvm/prebuilt/$(uname -s | tr '[:upper:]' '[:lower:]')-x86_64/bin
export CC_aarch64_linux_android=$qdp_toolchain/$QDP_ANDROID_TARGET$QDP_ANDROID_API-clang
export CXX_aarch64_linux_android=$qdp_toolchain/$QDP_ANDROID_TARGET$QDP_ANDROID_API-clang++
export AR_aarch64_linux_android=$qdp_toolchain/llvm-ar
export CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER=$qdp_toolchain/$QDP_ANDROID_TARGET$QDP_ANDROID_API-clang
unset qdp_ndk qdp_toolchain
