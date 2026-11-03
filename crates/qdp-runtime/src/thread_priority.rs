//! The priority of the threads that carry video to the screen: receiving
//! it, decoding it and drawing it.
//!
//! Apple's systems run a thread no one gave a quality of service at the
//! default one, below the interface's, and may run it on an efficiency core
//! at a low clock. A frame's decoding, which waits on VideoToolbox's
//! service, inherits its thread's quality of service, so a frame decoded on
//! such a thread waits longer for the CPU, and for the decoder, than the
//! interface would.

#![allow(unsafe_code)]

/// Asks the system to run the current thread as promptly as the user's
/// interface: at the user-interactive quality of service on Apple's
/// systems. Elsewhere, it leaves the thread as it is.
pub fn prioritize_current_thread() {
    #[cfg(target_vendor = "apple")]
    {
        // SAFETY: the call takes a quality of service class and a relative
        // priority, 0 for the class's own, and changes only the current
        // thread.
        let status = unsafe {
            libc::pthread_set_qos_class_self_np(libc::qos_class_t::QOS_CLASS_USER_INTERACTIVE, 0)
        };
        if status != 0 {
            tracing::debug!(status, "this thread's quality of service cannot be raised");
        }
    }
}
