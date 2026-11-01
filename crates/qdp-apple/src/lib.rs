//! The Apple apps' core of Quick Desktop Protocol (QDP): the iPhone, iPad
//! and Mac apps call it from Swift through the C interface in
//! `include/qdp.h`.
//!
//! A session is [`qdp_app`]'s, as the Android app's is. Its video is
//! decoded by VideoToolbox, or by rav1d where the device has no AV1
//! decoder, and drawn with Metal on a thread of its own into a layer added
//! over the one the app gives: the pictures go from the session's thread
//! straight to the render thread, never through the main thread, as in the
//! desktop client on macOS. The host's cursor is drawn over the picture
//! where the system draws no pointer, as for touch input on iOS.
//!
//! The C functions exist only on macOS and iOS; the placement of the
//! picture they use is [`qdp_app::placement`], tested with the workspace.

#[cfg(any(target_os = "macos", target_os = "ios"))]
mod ffi;
#[cfg(any(target_os = "macos", target_os = "ios"))]
mod stage;
