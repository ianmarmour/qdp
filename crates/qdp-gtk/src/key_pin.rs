//! Hosts' key pins, as `qdp` writes them: the SHA-256 hash of the host's
//! certificate key (Section 3.13), in 64 lowercase hexadecimal digits.

use std::fmt;

/// The number of hexadecimal digits in a key pin.
const PIN_DIGITS: usize = 64;

/// A host's key pin.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct KeyPin(String);

impl KeyPin {
    /// Reads `text` as a key pin, in either case, or returns `None` if it is
    /// not one.
    pub fn parse(text: &str) -> Option<Self> {
        let valid = text.len() == PIN_DIGITS && text.bytes().all(|byte| byte.is_ascii_hexdigit());
        valid.then(|| Self(text.to_ascii_lowercase()))
    }

    /// Returns the pin's digits, as `qdp` takes them.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for KeyPin {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_pins() {
        let pin = KeyPin::parse(&"AB".repeat(32)).unwrap();
        assert_eq!(pin.as_str(), "ab".repeat(32));
        assert!(KeyPin::parse("ab").is_none());
        assert!(KeyPin::parse(&"zz".repeat(32)).is_none());
        assert!(KeyPin::parse("-").is_none());
    }
}
