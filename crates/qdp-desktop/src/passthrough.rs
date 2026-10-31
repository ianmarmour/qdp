//! The client's passthrough devices: the HID devices the user chose, read
//! and written as the client's platform sees them (Section 8.2.5).
//!
//! Only Linux clients pass devices through yet, through hidraw.

use std::str::FromStr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;

use qdp_runtime::InputHandle;

/// A device the user chose to pass through, named by its USB or Bluetooth
/// vendor and product IDs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct DeviceSelector {
    /// The vendor ID.
    pub vendor_id: u16,
    /// The product ID.
    pub product_id: u16,
}

impl DeviceSelector {
    /// Returns whether a device with these IDs is the one chosen.
    pub fn matches(self, vendor_id: u16, product_id: u16) -> bool {
        (self.vendor_id, self.product_id) == (vendor_id, product_id)
    }
}

impl FromStr for DeviceSelector {
    type Err = String;

    /// Parses `VENDOR:PRODUCT`, both in hexadecimal, as `lsusb` shows them.
    fn from_str(text: &str) -> Result<Self, Self::Err> {
        let invalid = || format!("{text} is not VENDOR:PRODUCT in hexadecimal, as 054c:09cc");
        let (vendor, product) = text.split_once(':').ok_or_else(invalid)?;
        let id = |part: &str| u16::from_str_radix(part, 16).map_err(|_| invalid());
        Ok(Self {
            vendor_id: id(vendor)?,
            product_id: id(product)?,
        })
    }
}

/// Passes the chosen devices through until dropped.
#[derive(Debug)]
pub struct PassthroughCapture {
    /// Whether the window has focus, so that reports are sent.
    focused: Arc<AtomicBool>,
    /// Raised to stop the threads.
    stop: Arc<AtomicBool>,
    /// The threads reading the devices and passing the host's output on.
    threads: Vec<JoinHandle<()>>,
}

impl PassthroughCapture {
    /// Attaches the connected devices `selectors` name and starts passing
    /// them through, sending their input to `input`.
    pub fn start(input: &InputHandle, selectors: &[DeviceSelector]) -> Self {
        let focused = Arc::new(AtomicBool::new(false));
        let stop = Arc::new(AtomicBool::new(false));
        #[cfg(target_os = "linux")]
        let threads = crate::linux::start_passthrough(input, selectors, &focused, &stop);
        #[cfg(not(target_os = "linux"))]
        let threads = {
            let _ = (input, selectors);
            if !selectors.is_empty() {
                tracing::warn!("this client cannot pass devices through");
            }
            Vec::new()
        };
        Self {
            focused,
            stop,
            threads,
        }
    }

    /// Tells the capture whether the window has focus: reports are sent
    /// only while it has (Section 8.6, rule 7).
    pub fn set_focused(&self, focused: bool) {
        self.focused.store(focused, Ordering::Relaxed);
    }
}

impl Drop for PassthroughCapture {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        for thread in self.threads.drain(..) {
            let _ = thread.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Selectors parse from `VENDOR:PRODUCT` in hexadecimal.
    #[test]
    fn parses_selectors() {
        let selector: DeviceSelector = "054c:09CC".parse().unwrap();
        assert!(selector.matches(0x054c, 0x09cc));
        assert!(!selector.matches(0x054c, 0x05c4));
        assert!("054c".parse::<DeviceSelector>().is_err());
        assert!("054c:zz".parse::<DeviceSelector>().is_err());
    }
}
